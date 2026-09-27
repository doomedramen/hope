# Project rules

## UI and UX: useful information first

Every page must show what a user would expect to see before any interaction or scrolling on that page.

- Identify the user's main question and task for the page. Show the answer, current state, and relevant primary action in the initial viewport.
- Put important status, current readings, freshness, exceptions, and the next useful action ahead of technical metadata or configuration.
- Do not require opening a tab, expanding a panel, selecting a row, hovering, or scrolling to discover information essential to the page's purpose.
- Reserve progressive disclosure for secondary details, history, technical identifiers, advanced controls, and supporting evidence.
- Keep headers, navigation, spacing, and repeated labels compact enough to leave room for useful content. Do not shrink text below readable sizes to force content to fit.
- Show honest loading, empty, stale, unavailable, and error states in the same prominent positions. Never make missing or stale data look healthy.
- Verify the initial viewport at desktop and mobile widths with realistic data, including long names and failure states. Primary status, essential information, and the next action must remain visible and readable without interaction or scrolling.
- A page is not complete until this initial-view check passes. When content competes for space, prioritize what the user needs to decide or do now.

## Collaboration and Git

- Use concise communication (`/caveman`). Write persisted documentation and code in normal prose.
- Work in the currently checked out branch. Before using another branch or worktree, ask the user and wait for permission.
- Commit work to Git and push regularly.
- Never add co-author attribution to commits.
- If sub-agents are explicitly authorized, use only `gpt-6-luna` with `reasoning_effort: max` unless the user says otherwise.
