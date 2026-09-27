-- Bound per-agent recent usage sampling even when the fleet has a large retained history.
create index agent_metrics_agent_received on agent_metric_samples(agent_id,received_at desc);
