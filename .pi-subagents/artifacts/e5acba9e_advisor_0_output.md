skill_catalogue_seen:
- delegate-task: yes — "Delegate one complex task to a single subagent, review its work in two stages before merging back."
- request-review: yes — "Dispatch a fresh reviewer agent with a clean context to critique the code after audit-code passes."
- diagnose-stall: yes — "Diagnose why agent orchestration stopped producing progress — silent stalls in /loop, dispatch-agents, or execute-plan."
- research-first: yes — "Look-before-build — search registries, repo, existing skills, and web for prior art before implementing."