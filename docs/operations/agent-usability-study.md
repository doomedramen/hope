# Agent workflow usability study

Status: ready to run; no human participants or results have been recorded.

The [self-guided study pack](agent-study-pack/START-HERE.md) includes participant tasks, a session sheet, individual response forms, facilitator setup and success criteria, 25 empty result rows for five participants, and a findings template. The owner must prepare isolated fixtures and supply temporary access before sessions can start. Share only the participant folder with participants.

Recruit five representative operators with a mix of Linux and Hope experience. Use a disposable environment containing a healthy agent, a stale agent, a source with denied access, and a known resource spike with matching journal entries. Include a 375-pixel mobile viewport and keyboard-only navigation. Do not use production credentials or ask participants to share secrets.

Give each participant these tasks without describing navigation steps:

1. Install an agent and determine when setup is complete. Observe whether they verify the command's own agent and distinguish optional logs from required telemetry.
2. Explain why an agent's readings are old. Observe whether they distinguish connection state, collection failure, buffered delivery and historical values.
3. Find a resource spike and its related logs. Observe time-range preservation, severity/source interpretation and ability to pause while reading.
4. Limit a noisy log source, identify resulting gaps, and confirm that the agent applied the change.
5. Request an update and explain a rollback outcome, without assuming that a queued request succeeded.

For each task record elapsed time, completion without help, wrong destinations, hidden essential information, misunderstood states, keyboard barriers and participant wording. Ask what they expected to see before they scrolled or interacted. Do not count a facilitated walkthrough as an unassisted success.

| Participant | Task | Completed unaided | Seconds | Wrong destinations | Misunderstood state | Initial viewport issue | Follow-up |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Pending | | | | | | | |

After all sessions, group recurring failures, prioritize corrections by task impact, and repeat affected tasks. Report participant evidence separately from automated and agent-driven browser checks; neither substitutes for human usability results.
