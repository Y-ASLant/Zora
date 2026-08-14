use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::sftp_backend::SftpBackend;
use super::sftp_ops::SftpOpsError;
use super::types::{FileEntryType, RemoteFileVersion};

pub const DEFAULT_AUTO_OPEN_TEXT_MAX_BYTES: u64 = 8 * 1024 * 1024;
pub const DEFAULT_TEXT_CACHE_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_SNIFF_BYTES: u64 = 64 * 1024;

#[derive(Clone)]
pub struct RemoteFileService {
    auto_open_text_max_bytes: u64,
    text_cache_max_bytes: u64,
    sniff_bytes: u64,
    text_cache: Arc<Mutex<RemoteTextCache>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteFileEncoding {
    Utf8,
    Utf8Bom,
    Utf16LeBom,
    Utf16BeBom,
}

#[derive(Debug, Clone)]
pub struct RemoteTextFile {
    pub text: String,
    pub version: RemoteFileVersion,
    pub encoding: RemoteFileEncoding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteTextCacheKey {
    provider_id: String,
    path: PathBuf,
    version: RemoteFileVersion,
}

#[derive(Debug, Clone)]
struct RemoteTextCacheEntry {
    key: RemoteTextCacheKey,
    file: RemoteTextFile,
    size_bytes: u64,
}

#[derive(Debug)]
struct RemoteTextCache {
    max_bytes: u64,
    current_bytes: u64,
    entries: VecDeque<RemoteTextCacheEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteOpenDecision {
    Text(RemoteFileEncoding),
    TooLarge {
        size: u64,
        max: u64,
        preview: Option<String>,
    },
    Binary,
    UnsupportedFileType(FileEntryType),
    UnsupportedEncoding,
}

impl Default for RemoteFileService {
    fn default() -> Self {
        Self {
            auto_open_text_max_bytes: DEFAULT_AUTO_OPEN_TEXT_MAX_BYTES,
            text_cache_max_bytes: DEFAULT_TEXT_CACHE_MAX_BYTES,
            sniff_bytes: DEFAULT_SNIFF_BYTES,
            text_cache: Arc::new(Mutex::new(RemoteTextCache::new(
                DEFAULT_TEXT_CACHE_MAX_BYTES,
            ))),
        }
    }
}

impl RemoteFileService {
    pub fn new(auto_open_text_max_bytes: u64, sniff_bytes: u64) -> Self {
        Self::new_with_cache(
            auto_open_text_max_bytes,
            sniff_bytes,
            DEFAULT_TEXT_CACHE_MAX_BYTES,
        )
    }

    pub fn new_with_cache(
        auto_open_text_max_bytes: u64,
        sniff_bytes: u64,
        text_cache_max_bytes: u64,
    ) -> Self {
        Self {
            auto_open_text_max_bytes,
            text_cache_max_bytes,
            sniff_bytes,
            text_cache: Arc::new(Mutex::new(RemoteTextCache::new(text_cache_max_bytes))),
        }
    }

    pub fn auto_open_text_max_bytes(&self) -> u64 {
        self.auto_open_text_max_bytes
    }

    pub fn text_cache_max_bytes(&self) -> u64 {
        self.text_cache_max_bytes
    }

    pub fn configure_limits(
        &mut self,
        auto_open_text_max_bytes: u64,
        sniff_bytes: u64,
        text_cache_max_bytes: u64,
    ) {
        self.auto_open_text_max_bytes = auto_open_text_max_bytes;
        self.sniff_bytes = sniff_bytes;
        self.text_cache_max_bytes = text_cache_max_bytes;
        if let Ok(mut cache) = self.text_cache.lock() {
            cache.set_max_bytes(text_cache_max_bytes);
        } else {
            log::warn!("Failed to update remote text cache budget");
        }
    }

    pub fn clone_with_auto_open_text_max_bytes(&self, auto_open_text_max_bytes: u64) -> Self {
        Self {
            auto_open_text_max_bytes,
            text_cache_max_bytes: self.text_cache_max_bytes,
            sniff_bytes: self.sniff_bytes,
            text_cache: self.text_cache.clone(),
        }
    }

    pub fn decide_open(
        &self,
        backend: &dyn SftpBackend,
        path: &Path,
    ) -> Result<RemoteOpenDecision, SftpOpsError> {
        let stat = backend.stat(path)?;
        if !matches!(stat.file_type, FileEntryType::File) {
            return Ok(RemoteOpenDecision::UnsupportedFileType(stat.file_type));
        }
        let sniff_len = stat.size.min(self.sniff_bytes);
        let prefix = backend.read_file_range(path, 0, sniff_len)?;
        let decision = classify_text_bytes(&prefix);
        if stat.size > self.auto_open_text_max_bytes {
            return Ok(match decision {
                RemoteOpenDecision::Text(encoding) => RemoteOpenDecision::TooLarge {
                    size: stat.size,
                    max: self.auto_open_text_max_bytes,
                    preview: decode_text(&prefix, &encoding)
                        .ok()
                        .map(|text| truncate_preview(&text, 800)),
                },
                RemoteOpenDecision::TooLarge { .. } => unreachable!(),
                RemoteOpenDecision::Binary => RemoteOpenDecision::Binary,
                RemoteOpenDecision::UnsupportedFileType(file_type) => {
                    RemoteOpenDecision::UnsupportedFileType(file_type)
                }
                RemoteOpenDecision::UnsupportedEncoding => RemoteOpenDecision::UnsupportedEncoding,
            });
        }
        Ok(decision)
    }

    pub fn open_text(
        &self,
        backend: Arc<dyn SftpBackend>,
        path: PathBuf,
    ) -> Result<RemoteTextFile, SftpOpsError> {
        match self.decide_open(backend.as_ref(), &path)? {
            RemoteOpenDecision::Text(encoding) => {
                let file = backend.read_file(&path, self.auto_open_text_max_bytes)?;
                if !matches!(file.file_type, FileEntryType::File) {
                    return Err(SftpOpsError::Operation(format!(
                        "不是普通文件: {}",
                        path.display()
                    )));
                }
                let text = decode_text(&file.bytes, &encoding).map_err(SftpOpsError::Operation)?;
                Ok(RemoteTextFile {
                    text,
                    version: file.version,
                    encoding,
                })
            }
            RemoteOpenDecision::TooLarge { size, max, .. } => {
                Err(SftpOpsError::FileTooLarge { size, max })
            }
            RemoteOpenDecision::Binary => Err(SftpOpsError::Operation(format!(
                "文件看起来是二进制文件: {}",
                path.display()
            ))),
            RemoteOpenDecision::UnsupportedFileType(file_type) => Err(SftpOpsError::Operation(
                format!("不支持打开该远程条目类型: {file_type:?}"),
            )),
            RemoteOpenDecision::UnsupportedEncoding => Err(SftpOpsError::Operation(format!(
                "无法作为 UTF-8/UTF-16 文本安全打开: {}",
                path.display()
            ))),
        }
    }

    pub fn open_text_cached(
        &self,
        provider_id: String,
        backend: Arc<dyn SftpBackend>,
        path: PathBuf,
    ) -> Result<RemoteTextFile, SftpOpsError> {
        let version = backend.file_version(&path)?;
        let key = RemoteTextCacheKey {
            provider_id: provider_id.clone(),
            path: path.clone(),
            version,
        };
        if let Some(file) = self.cached_text(&key)? {
            return Ok(file);
        }

        let file = self.open_text(backend, path.clone())?;
        self.store_cached_text(
            RemoteTextCacheKey {
                provider_id,
                path,
                version: file.version,
            },
            &file,
        )?;
        Ok(file)
    }

    pub fn cache_text(&self, provider_id: String, path: PathBuf, file: RemoteTextFile) {
        let key = RemoteTextCacheKey {
            provider_id,
            path,
            version: file.version,
        };
        if let Err(error) = self.store_cached_text(key, &file) {
            log::warn!("Failed to update remote text cache: {error}");
        }
    }

    pub fn clear_cache(&self) {
        match self.text_cache.lock() {
            Ok(mut cache) => cache.clear(),
            Err(error) => log::warn!("Failed to clear remote text cache: {error}"),
        }
    }

    pub fn encode_text(text: &str, encoding: &RemoteFileEncoding) -> Vec<u8> {
        match encoding {
            RemoteFileEncoding::Utf8 => text.as_bytes().to_vec(),
            RemoteFileEncoding::Utf8Bom => {
                let mut bytes = vec![0xEF, 0xBB, 0xBF];
                bytes.extend_from_slice(text.as_bytes());
                bytes
            }
            RemoteFileEncoding::Utf16LeBom => {
                let mut bytes = vec![0xFF, 0xFE];
                bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
                bytes
            }
            RemoteFileEncoding::Utf16BeBom => {
                let mut bytes = vec![0xFE, 0xFF];
                bytes.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
                bytes
            }
        }
    }

    fn cached_text(
        &self,
        key: &RemoteTextCacheKey,
    ) -> Result<Option<RemoteTextFile>, SftpOpsError> {
        self.text_cache
            .lock()
            .map_err(|error| SftpOpsError::Operation(format!("远程文本缓存锁失败: {error}")))
            .map(|mut cache| cache.get(key))
    }

    fn store_cached_text(
        &self,
        key: RemoteTextCacheKey,
        file: &RemoteTextFile,
    ) -> Result<(), SftpOpsError> {
        self.text_cache
            .lock()
            .map_err(|error| SftpOpsError::Operation(format!("远程文本缓存锁失败: {error}")))
            .map(|mut cache| cache.insert(key, file.clone()))
    }
}

impl RemoteTextCache {
    fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            current_bytes: 0,
            entries: VecDeque::new(),
        }
    }

    fn get(&mut self, key: &RemoteTextCacheKey) -> Option<RemoteTextFile> {
        let index = self.entries.iter().position(|entry| &entry.key == key)?;
        let entry = self.entries.remove(index)?;
        let file = entry.file.clone();
        self.entries.push_back(entry);
        Some(file)
    }

    fn set_max_bytes(&mut self, max_bytes: u64) {
        self.max_bytes = max_bytes;
        if max_bytes == 0 {
            self.clear();
        } else {
            self.evict_until_within_budget();
        }
    }

    fn insert(&mut self, key: RemoteTextCacheKey, file: RemoteTextFile) {
        if self.max_bytes == 0 {
            self.clear();
            return;
        }

        let size_bytes = u64::try_from(file.text.len()).unwrap_or(u64::MAX);
        if size_bytes > self.max_bytes {
            return;
        }

        if let Some(index) = self.entries.iter().position(|entry| entry.key == key) {
            if let Some(entry) = self.entries.remove(index) {
                self.current_bytes = self.current_bytes.saturating_sub(entry.size_bytes);
            }
        }

        self.current_bytes = self.current_bytes.saturating_add(size_bytes);
        self.entries.push_back(RemoteTextCacheEntry {
            key,
            file,
            size_bytes,
        });
        self.evict_until_within_budget();
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.current_bytes = 0;
    }

    fn evict_until_within_budget(&mut self) {
        while self.current_bytes > self.max_bytes {
            let Some(entry) = self.entries.pop_front() else {
                self.current_bytes = 0;
                return;
            };
            self.current_bytes = self.current_bytes.saturating_sub(entry.size_bytes);
        }
    }
}

pub fn classify_text_bytes(bytes: &[u8]) -> RemoteOpenDecision {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return RemoteOpenDecision::Text(RemoteFileEncoding::Utf8Bom);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return RemoteOpenDecision::Text(RemoteFileEncoding::Utf16LeBom);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return RemoteOpenDecision::Text(RemoteFileEncoding::Utf16BeBom);
    }
    if bytes.contains(&0) || looks_binary_control_heavy(bytes) {
        return RemoteOpenDecision::Binary;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => RemoteOpenDecision::Text(RemoteFileEncoding::Utf8),
        Err(_) => RemoteOpenDecision::UnsupportedEncoding,
    }
}

fn looks_binary_control_heavy(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let control_count = bytes
        .iter()
        .filter(|byte| matches!(**byte, 0x01..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F))
        .count();
    control_count * 100 / bytes.len() > 10
}

fn decode_text(bytes: &[u8], encoding: &RemoteFileEncoding) -> Result<String, String> {
    match encoding {
        RemoteFileEncoding::Utf8 => {
            String::from_utf8(bytes.to_vec()).map_err(|err| err.to_string())
        }
        RemoteFileEncoding::Utf8Bom => {
            let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
            String::from_utf8(bytes.to_vec()).map_err(|err| err.to_string())
        }
        RemoteFileEncoding::Utf16LeBom => decode_utf16_bom(bytes, true),
        RemoteFileEncoding::Utf16BeBom => decode_utf16_bom(bytes, false),
    }
}

fn truncate_preview(text: &str, max_chars: usize) -> String {
    let mut preview: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        preview.push_str("\n...");
    }
    preview
}

fn decode_utf16_bom(bytes: &[u8], little_endian: bool) -> Result<String, String> {
    let bytes = bytes.get(2..).unwrap_or_default();
    if bytes.len() % 2 != 0 {
        return Err("UTF-16 字节长度不是偶数".to_string());
    }
    let units = bytes.chunks_exact(2).map(|chunk| {
        if little_endian {
            u16::from_le_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], chunk[1]])
        }
    });
    char::decode_utf16(units)
        .collect::<Result<String, _>>()
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sftp_manager::sftp_backend::InMemorySftpBackend;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zora-sftp-remote-file-service-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cache_key(provider_id: &str, path: &str, size: u64) -> RemoteTextCacheKey {
        RemoteTextCacheKey {
            provider_id: provider_id.to_string(),
            path: PathBuf::from(path),
            version: RemoteFileVersion {
                size,
                modified: None,
            },
        }
    }

    fn cached_text(text: &str) -> RemoteTextFile {
        RemoteTextFile {
            text: text.to_string(),
            version: RemoteFileVersion {
                size: text.len() as u64,
                modified: None,
            },
            encoding: RemoteFileEncoding::Utf8,
        }
    }

    #[test]
    fn classifies_utf8_text() {
        assert_eq!(
            classify_text_bytes(b"hello\nworld"),
            RemoteOpenDecision::Text(RemoteFileEncoding::Utf8)
        );
    }

    #[test]
    fn classifies_bom_encodings() {
        assert_eq!(
            classify_text_bytes(&[0xEF, 0xBB, 0xBF, b'a']),
            RemoteOpenDecision::Text(RemoteFileEncoding::Utf8Bom)
        );
        assert_eq!(
            classify_text_bytes(&[0xFF, 0xFE, b'a', 0]),
            RemoteOpenDecision::Text(RemoteFileEncoding::Utf16LeBom)
        );
        assert_eq!(
            classify_text_bytes(&[0xFE, 0xFF, 0, b'a']),
            RemoteOpenDecision::Text(RemoteFileEncoding::Utf16BeBom)
        );
    }

    #[test]
    fn classifies_nul_as_binary() {
        assert_eq!(classify_text_bytes(b"abc\0def"), RemoteOpenDecision::Binary);
    }

    #[test]
    fn classifies_invalid_utf8_as_unsupported_encoding() {
        assert_eq!(
            classify_text_bytes(&[0xFF, 0xFE, 0xFF]),
            RemoteOpenDecision::Text(RemoteFileEncoding::Utf16LeBom)
        );
        assert_eq!(
            classify_text_bytes(&[0xC3, 0x28]),
            RemoteOpenDecision::UnsupportedEncoding
        );
    }

    #[test]
    fn opens_utf16le_bom_text() {
        let root = temp_dir();
        let path = root.join("hello.txt");
        fs::write(&path, [0xFF, 0xFE, b'h', 0, b'i', 0]).unwrap();
        let backend = Arc::new(InMemorySftpBackend::new(root.clone()));
        let service = RemoteFileService::default();

        let opened = service
            .open_text(backend, PathBuf::from("hello.txt"))
            .unwrap();
        assert_eq!(opened.text, "hi");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn large_file_returns_too_large_decision() {
        let root = temp_dir();
        let path = root.join("large.log");
        fs::write(&path, b"abcdef").unwrap();
        let backend = InMemorySftpBackend::new(root.clone());
        let service = RemoteFileService::new(4, 4);

        assert_eq!(
            service
                .decide_open(&backend, Path::new("large.log"))
                .unwrap(),
            RemoteOpenDecision::TooLarge {
                size: 6,
                max: 4,
                preview: Some("abcd".to_string())
            }
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn large_binary_returns_binary_decision() {
        let root = temp_dir();
        let path = root.join("large.bin");
        fs::write(&path, b"abc\0def").unwrap();
        let backend = InMemorySftpBackend::new(root.clone());
        let service = RemoteFileService::new(4, 4);

        assert_eq!(
            service
                .decide_open(&backend, Path::new("large.bin"))
                .unwrap(),
            RemoteOpenDecision::Binary
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn large_file_preview_is_truncated() {
        let root = temp_dir();
        let path = root.join("large.log");
        let content = "a".repeat(900);
        fs::write(&path, content).unwrap();
        let backend = InMemorySftpBackend::new(root.clone());
        let service = RemoteFileService::new(4, 900);

        let decision = service
            .decide_open(&backend, Path::new("large.log"))
            .unwrap();
        let RemoteOpenDecision::TooLarge {
            preview: Some(preview),
            ..
        } = decision
        else {
            panic!("expected large text preview");
        };
        assert_eq!(preview.chars().count(), 804);
        assert!(preview.ends_with("\n..."));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_matches_provider_path_and_version() {
        let mut cache = RemoteTextCache::new(64);
        let key = cache_key("node-a", "/etc/hosts", 2);
        cache.insert(key.clone(), cached_text("aa"));

        assert!(cache.get(&cache_key("node-b", "/etc/hosts", 2)).is_none());
        assert!(cache.get(&cache_key("node-a", "/etc/passwd", 2)).is_none());
        assert!(cache.get(&cache_key("node-a", "/etc/hosts", 3)).is_none());
        assert_eq!(cache.get(&key).unwrap().text, "aa");
    }

    #[test]
    fn cache_evicts_least_recently_used_entry() {
        let mut cache = RemoteTextCache::new(4);
        let a = cache_key("node-a", "/a", 2);
        let b = cache_key("node-a", "/b", 2);
        let c = cache_key("node-a", "/c", 2);

        cache.insert(a.clone(), cached_text("aa"));
        cache.insert(b.clone(), cached_text("bb"));
        assert_eq!(cache.get(&a).unwrap().text, "aa");

        cache.insert(c.clone(), cached_text("cc"));

        assert!(cache.get(&b).is_none());
        assert_eq!(cache.get(&a).unwrap().text, "aa");
        assert_eq!(cache.get(&c).unwrap().text, "cc");
    }

    #[test]
    fn cache_budget_zero_disables_cache() {
        let mut cache = RemoteTextCache::new(0);
        let key = cache_key("node-a", "/a", 2);

        cache.insert(key.clone(), cached_text("aa"));

        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn service_reconfigures_cache_budget() {
        let mut service = RemoteFileService::new_with_cache(8, 4, 8);
        let a = cache_key("node-a", "/a", 4);
        let b = cache_key("node-a", "/b", 4);

        service.cache_text(
            "node-a".to_string(),
            PathBuf::from("/a"),
            cached_text("aaaa"),
        );
        service.cache_text(
            "node-a".to_string(),
            PathBuf::from("/b"),
            cached_text("bbbb"),
        );
        service.configure_limits(8, 4, 4);

        assert_eq!(service.text_cache_max_bytes(), 4);
        let mut cache = service.text_cache.lock().unwrap();
        assert!(cache.get(&a).is_none());
        assert_eq!(cache.get(&b).unwrap().text, "bbbb");
    }

    #[test]
    fn clone_with_open_limit_shares_cache() {
        let service = RemoteFileService::new_with_cache(8, 4, 16);
        let clone = service.clone_with_auto_open_text_max_bytes(32);
        let key = cache_key("node-a", "/shared", 6);

        clone.cache_text(
            "node-a".to_string(),
            PathBuf::from("/shared"),
            cached_text("shared"),
        );

        assert_eq!(service.auto_open_text_max_bytes(), 8);
        assert_eq!(clone.auto_open_text_max_bytes(), 32);
        assert_eq!(service.cached_text(&key).unwrap().unwrap().text, "shared");
    }

    #[test]
    fn encode_text_preserves_bom_encodings() {
        assert_eq!(
            RemoteFileService::encode_text("hi", &RemoteFileEncoding::Utf8Bom),
            [0xEF, 0xBB, 0xBF, b'h', b'i']
        );
        assert_eq!(
            RemoteFileService::encode_text("hi", &RemoteFileEncoding::Utf16LeBom),
            [0xFF, 0xFE, b'h', 0, b'i', 0]
        );
        assert_eq!(
            RemoteFileService::encode_text("hi", &RemoteFileEncoding::Utf16BeBom),
            [0xFE, 0xFF, 0, b'h', 0, b'i']
        );
    }
}
