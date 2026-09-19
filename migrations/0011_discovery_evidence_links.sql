-- Milestone 2: link a persisted scan observation to the evidence row it
-- produced. Closed/filtered observations may remain unlinked when a run is
-- partial; complete-run absence evidence is linked during reconciliation.

alter table port_observations
    add column if not exists evidence_id uuid references evidence(id);

create index if not exists port_observations_evidence_idx
    on port_observations (evidence_id)
    where evidence_id is not null;
