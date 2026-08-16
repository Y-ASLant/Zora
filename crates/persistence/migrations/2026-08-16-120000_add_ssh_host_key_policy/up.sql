ALTER TABLE ssh_servers
ADD COLUMN host_key_policy TEXT NOT NULL DEFAULT 'known_hosts' CHECK(host_key_policy IN ('known_hosts','accept_any'));
