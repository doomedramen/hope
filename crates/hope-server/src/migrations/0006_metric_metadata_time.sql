-- The rollup timestamp is the start of a bucket, not the time of the latest
-- observed state/counter metadata. Keep the latter when merging late reports.
ALTER TABLE metrics ADD COLUMN latest_collected_at REAL;
