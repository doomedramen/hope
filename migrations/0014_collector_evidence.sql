-- Milestone 3: evidence from bounded mDNS/DNS-SD and UPnP/SSDP collectors.
-- The adapter records UPnP observations without treating LOCATION as a fetch
-- instruction. Evidence remains append-only and source-specific.

alter table evidence drop constraint if exists evidence_source_type_check;

alter table evidence add constraint evidence_source_type_check check (source_type in
    ('network_scan', 'agent', 'docker', 'mdns', 'upnp', 'snmp', 'api', 'manual'));
