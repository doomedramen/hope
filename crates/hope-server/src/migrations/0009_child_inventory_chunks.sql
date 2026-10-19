ALTER TABLE child_batches ADD COLUMN inventory_id TEXT;
ALTER TABLE child_batches ADD COLUMN chunk_index INTEGER NOT NULL DEFAULT 0;
ALTER TABLE child_batches ADD COLUMN chunk_count INTEGER NOT NULL DEFAULT 1;
CREATE INDEX child_batches_inventory_group
    ON child_batches(parent_agent_id,source,source_scope,inventory_id,inventory_generation);
