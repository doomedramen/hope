# Milestone 3 design: fingerprinting and service proposals

Spec refs: §4.3–4.4, §6.5–6.6, §8.2, §17 M3. M2 owns socket discovery,
safe protocol classification, and the canonical service/endpoint record. M3
adds explainable product evidence without replacing M2 records.

## 1. Boundary

M2 creates one `services` row and current socket endpoint for an open
device/address/port. Its `network_scan` evidence contains bounded HTTP, TLS,
SSH, or generic-TCP protocol facts. M3 consumes those facts and adds product
guesses, confidence, and reasons. It does not create a second service model.

Manual service fields and confirmed evidence win over every automatic rule.
An automatic result may enrich or revise automatic data only when its evidence
is at least as strong and is not contradicted by a confirmed user fact.

## 2. Fingerprint rule interface

Rules are pure functions over bounded normalized inputs. Each result contains:

- rule identifier and fixture version;
- protocol/product/version guess;
- confidence in `[0, 1]`;
- the exact evidence fields it used; and
- a stable explanation suitable for the UI's **Why?** view.

Rules never fetch extra URLs, authenticate, follow cross-endpoint redirects,
or inspect unbounded body data. Protocol collectors remain separately bounded.
The first set covers generic HTTP/TLS/SSH facts produced by M2, with generic
fallbacks when no signature matches.

## 3. Reconciliation and review

Automatic fingerprint evidence is append-only. A reconciliation transaction
selects the best supported automatic classification for a canonical service,
updates only automatic fields, and emits one material change event. Conflicting
or low-confidence guesses create a service-review item rather than silently
overwriting a service. The item lists competing candidates and their evidence.

The review UI must show the source observations, scores, and rules. Confirming
a classification writes manual evidence; rejecting it records the decision and
prevents the same unchanged automatic candidate from repeatedly resurfacing.

## 4. Monitor proposals

Rules propose monitors from a resolved protocol/product, but do not execute
checks. A proposal records its generating rule, target endpoint, confidence,
and whether policy permits automatic creation. User edits become explicit
overrides so later discovery cannot erase them.

M3 supports proposal persistence and review only. Continuous scheduling and
health execution are M4 work.

## 5. Delivery sequence

1. Pure rule engine, signed-in fixtures, and score/explanation tests.
2. Fingerprint evidence persistence and canonical service reconciliation.
3. Service-review queue and **Why?** API/UI.
4. Monitor-proposal policy, persistence, and review UI.
5. Safe collector adapters for mDNS/DNS-SD and UPnP, then controlled SNMP.

## 6. Acceptance evidence

M3 is complete only when repeated discoveries enrich one service; unknown
HTTP/TCP remain visible and receive generic proposals; every fingerprint shows
evidence and confidence; and weaker scans cannot overwrite user confirmation.
