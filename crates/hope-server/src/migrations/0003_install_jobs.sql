-- Nonsecret SSH installation job metadata. Passwords, keys, passphrases, enrollment tokens and raw remote output are never stored.
CREATE TABLE install_jobs (
    id TEXT PRIMARY KEY,
    host_id TEXT NOT NULL,
    address TEXT NOT NULL,
    port INTEGER NOT NULL,
    fingerprint TEXT NOT NULL,
    username TEXT NOT NULL,
    mode TEXT NOT NULL,
    status TEXT NOT NULL,
    stage TEXT,
    stages TEXT NOT NULL DEFAULT '[]',
    error_code TEXT,
    message TEXT,
    notice TEXT,
    bound_agent_id TEXT,
    agent_id TEXT,
    created_at REAL NOT NULL,
    finished_at REAL
);
CREATE INDEX install_jobs_created ON install_jobs(created_at);
CREATE INDEX install_jobs_status ON install_jobs(status);
