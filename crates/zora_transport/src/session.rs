use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::error::{Result, TransportError, ssh_error};
use crate::sftp::Sftp;

/// SSH 认证方式。
#[derive(Clone, Debug)]
pub enum AuthMethod {
    Password {
        password: String,
    },
    PublicKey {
        key_path: PathBuf,
        passphrase: Option<String>,
    },
}

/// SSH 服务器公钥校验策略。
#[derive(Clone)]
pub enum ServerKeyPolicy {
    AcceptAny,
    KnownHosts,
    KnownHostsFile(PathBuf),
    Custom(ServerKeyVerifier),
}

pub type ServerKeyVerifier = Arc<dyn Fn(&russh::keys::ssh_key::PublicKey) -> bool + Send + Sync>;

impl Default for ServerKeyPolicy {
    fn default() -> Self {
        Self::KnownHosts
    }
}

impl std::fmt::Debug for ServerKeyPolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AcceptAny => formatter.write_str("AcceptAny"),
            Self::KnownHosts => formatter.write_str("KnownHosts"),
            Self::KnownHostsFile(path) => {
                formatter.debug_tuple("KnownHostsFile").field(path).finish()
            }
            Self::Custom(_) => formatter.write_str("Custom(..)"),
        }
    }
}

impl ServerKeyPolicy {
    fn verifier(&self, host: &str, port: u16) -> ServerKeyVerifier {
        match self {
            Self::AcceptAny => Arc::new(|_| true),
            Self::KnownHosts => {
                let host = host.to_owned();
                Arc::new(move |key| {
                    russh::keys::check_known_hosts(&host, port, key).unwrap_or(false)
                })
            }
            Self::KnownHostsFile(path) => {
                let host = host.to_owned();
                let path = path.clone();
                Arc::new(move |key| {
                    russh::keys::check_known_hosts_path(&host, port, key, &path).unwrap_or(false)
                })
            }
            Self::Custom(verifier) => Arc::clone(verifier),
        }
    }
}

#[derive(Clone)]
struct ClientHandler {
    verifier: ServerKeyVerifier,
}

impl russh::client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        Ok((self.verifier)(server_public_key))
    }
}

/// 已认证的 SFTP 会话。
pub struct SftpSession {
    sftp: Sftp,
}

impl std::fmt::Debug for SftpSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SftpSession")
            .finish_non_exhaustive()
    }
}

impl SftpSession {
    pub async fn connect(
        host: &str,
        port: u16,
        username: &str,
        auth: AuthMethod,
        timeout: Option<Duration>,
    ) -> Result<Self> {
        Self::connect_with_policy(
            host,
            port,
            username,
            auth,
            timeout,
            ServerKeyPolicy::default(),
        )
        .await
    }

    pub async fn connect_with_policy(
        host: &str,
        port: u16,
        username: &str,
        auth: AuthMethod,
        timeout: Option<Duration>,
        policy: ServerKeyPolicy,
    ) -> Result<Self> {
        let host = host.to_owned();
        let username = username.to_owned();
        let connect = async move {
            let config = russh::client::Config {
                inactivity_timeout: Some(Duration::from_secs(30)),
                keepalive_interval: Some(Duration::from_secs(15)),
                ..Default::default()
            };
            let handler = ClientHandler {
                verifier: policy.verifier(&host, port),
            };
            let mut session =
                russh::client::connect(Arc::new(config), (host.as_str(), port), handler)
                    .await
                    .map_err(ssh_error)?;

            let authenticated = match auth {
                AuthMethod::Password { password } => session
                    .authenticate_password(username, password)
                    .await
                    .map_err(ssh_error)?
                    .success(),
                AuthMethod::PublicKey {
                    key_path,
                    passphrase,
                } => {
                    let private_key =
                        russh::keys::load_secret_key(&key_path, passphrase.as_deref()).map_err(
                            |error| TransportError::AuthenticationFailed(error.to_string()),
                        )?;
                    let hash = session
                        .best_supported_rsa_hash()
                        .await
                        .map_err(ssh_error)?
                        .flatten();
                    session
                        .authenticate_publickey(
                            username,
                            russh::keys::PrivateKeyWithHashAlg::new(Arc::new(private_key), hash),
                        )
                        .await
                        .map_err(ssh_error)?
                        .success()
                }
            };
            if !authenticated {
                return Err(TransportError::AuthenticationFailed(
                    "服务器拒绝了认证请求".to_string(),
                ));
            }

            let channel = session.channel_open_session().await.map_err(ssh_error)?;
            channel
                .request_subsystem(true, "sftp")
                .await
                .map_err(ssh_error)?;
            let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
                .await
                .map_err(crate::error::sftp_error)?;
            Ok(Self {
                sftp: Sftp::new(sftp),
            })
        };

        match timeout {
            Some(timeout) => tokio::time::timeout(timeout, connect)
                .await
                .map_err(|_| TransportError::Timeout)?,
            None => connect.await,
        }
    }

    pub fn sftp(&self) -> Sftp {
        self.sftp.clone()
    }

    pub async fn home_dir(&self) -> Result<PathBuf> {
        self.sftp.canonicalize(PathBuf::from(".")).await
    }

    pub async fn execute(&self, command: &str) -> Result<CommandOutput> {
        self.sftp.execute(command).await
    }
}

#[derive(Debug, Default)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<u32>,
}
