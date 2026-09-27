-- Run only in a disposable copy of the completed TLS soak database.
-- The fixed database-name guard prevents accidental execution in the preview.
\set ON_ERROR_STOP on
select current_database() = 'hope_storage_sizing'
    and to_regclass('public.storage_rollup_sample') is null
    and to_regclass('public.storage_rollup_varied') is null
    and (select count(*) from agents) = 500
    and not exists (select 1 from agents where agent_version is distinct from 'soak-test')
    as correct_database \gset
\if :correct_database
\else
  do $$ begin
    raise exception 'Use a fresh disposable database named hope_storage_sizing, cloned from the completed 500-agent soak fixture, without existing calibration tables.';
  end $$;
\endif

-- VACUUM FULL is safe only in this isolated copy. It establishes compact table
-- and index sizes; it is not a recommended production retention operation.
vacuum (full, analyze) agent_metric_samples, agent_log_entries, agent_metric_rollups;

-- Exclude the one-metric historical seed: it understates normal bucket width.
-- Include 20,000 mature full-payload buckets, with the real table/index layout.
create table storage_rollup_sample (like agent_metric_rollups including all);
insert into storage_rollup_sample
select r.agent_id, r.resolution, r.bucket_start + n * interval '1 day',
       r.sample_count, r.stats
from (
    select distinct on (agent_id, resolution) *
    from agent_metric_rollups
    where stats ? 'memory.used_percent'
    order by agent_id, resolution, sample_count desc
) r cross join generate_series(1,20) n;

-- A sensitivity fixture reduces compression gained from identical readings.
-- Preserve count/sum/average consistency and bounded minimum/latest/maximum.
-- These deterministic variations are synthetic, not observed host readings.
create table storage_rollup_varied (like agent_metric_rollups including all);
insert into storage_rollup_varied
select r.agent_id, r.resolution, r.bucket_start, r.sample_count,
       (select jsonb_object_agg(key, jsonb_build_object(
           'count', value->'count',
           'sum', average * (value->>'count')::double precision,
           'average', average,
           'minimum', least(average, (value->>'minimum')::double precision * 0.8),
           'maximum', greatest(average, (value->>'maximum')::double precision * 1.2),
           'latest', average,
           'last_at', extract(epoch from r.bucket_start)::bigint
       )) from (
           select key, value, (value->>'average')::double precision *
               (0.8 + ((hashtextextended(key || r.agent_id::text || r.bucket_start::text, 0) & 65535)::double precision / 65535.0) * 0.4) as average
           from jsonb_each(r.stats)
       ) varied)
from storage_rollup_sample r;

vacuum (full, analyze) storage_rollup_sample, storage_rollup_varied;

with measured_rows(relname, row_count) as (
    select 'agent_metric_samples', count(*) from agent_metric_samples
    union all select 'agent_log_entries', count(*) from agent_log_entries
    union all select 'storage_rollup_sample', count(*) from storage_rollup_sample
    union all select 'storage_rollup_varied', count(*) from storage_rollup_varied
)
select jsonb_pretty(jsonb_build_object(
    'schema_version', 1,
    'scope', 'Compact PostgreSQL 17 table and index measurements from an isolated copy of the synthetic 500-agent TLS soak; varied rollups are a compression sensitivity fixture.',
    'relations', (
        select jsonb_object_agg(relname, jsonb_build_object(
            'rows', row_count,
            'total_bytes', pg_total_relation_size(oid),
            'heap_bytes', pg_relation_size(oid),
            'index_bytes', pg_indexes_size(oid),
            'bytes_per_row', pg_total_relation_size(oid) / row_count::double precision
        )) from pg_class join measured_rows using (relname) where relnamespace = 'public'::regnamespace
        and relname in ('agent_metric_samples','agent_log_entries','storage_rollup_sample','storage_rollup_varied')
    )
));
