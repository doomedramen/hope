-- Independent of metric retention, downsampling and device identity reconciliation.
CREATE TABLE agent_report_heads (
    agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL CHECK(sequence > 0),
    report_id TEXT NOT NULL
);
