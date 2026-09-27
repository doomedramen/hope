# Self-guided agent usability study

Status: prepared, not conducted. This pack contains no participant results.

Use five real operators, labelled P01–P05. Allow about 35 minutes each. Each person should work independently and receive only the participant folder. Do not give participants the facilitator notes or success criteria before their session.

1. The study owner completes the isolated-environment checks in [Facilitator setup](facilitator/setup.md). Fill in the session sheet, verify each scenario, and reset the environment between participants.
2. Give each participant [Instructions and tasks](participant/tasks.md), a completed session sheet and a copy of [Response form](participant/response.md). No external service, account creation or recording is required by this pack.
3. Collect all five response forms. Transcribe observations into [Results](facilitator/results.csv), preserving participant wording. Leave unknowns blank; never turn an unanswered field into a success.
4. Use [Report template](facilitator/report.md) to identify repeated problems and decide which flows need correction and retesting.

Use an environment with disposable credentials and data. Never ask participants to perform enrollment, change collection rules or trigger rollback on monitored production hosts. Recording is optional and requires the participant's agreement. Do not put credentials, enrollment tokens or raw private logs in response forms.

The existing automated/browser checks establish software behavior; they do not count as any of these five sessions. An unfinished session remains unfinished. Share completed results with the development team for review; this pack does not contact anyone or upload responses.

## Session sheet — complete one per participant

| Field | Value |
| --- | --- |
| Participant ID | P01 / P02 / P03 / P04 / P05 |
| Date and time zone | |
| Application URL and tested commit | |
| Browser and viewport size | |
| Input method | Mouse/touch/keyboard |
| Disposable target to enroll | |
| Existing host for task 2 | |
| Existing host and investigation period for task 3 | |
| Existing host and selected source for task 4 | |
| Existing host and approved test release for task 5 | |
| Contact for a blocked or unsafe task | |

Credentials must be shared separately through the study owner's usual secure channel.
