# Devboule UI Pilot scenarios

| Scenario | What |
|----------|------|
| `smoke-session.sh` | Unlocked (strict) + list_projects + get_config |
| `list-projects.sh` | list + `get_project` with **projectId** |
| `run-chrome-shell.sh` | chrome TOML with **live socket** from env |
| `chrome-shell.toml` | /tmp only — prefer run-chrome-shell.sh |

```bash
./tools/devboule-pilot/up.sh --start-app
./tools/devboule-pilot/scenarios/smoke-session.sh
REQUIRE_MIN=1 ./tools/devboule-pilot/scenarios/list-projects.sh
./tools/devboule-pilot/scenarios/run-chrome-shell.sh
```
