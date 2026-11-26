# Custom window stats

A custom window can only show a stat that is a fight outcome, one the built-in windows already show. Gear, builds and anything ArenaNet disallows are never offered.

Each stat is one number per player. A window can list players or show a single total. Stats marked "per second" are fixed rates.

| Key | What it counts | Rows | Allowed because |
| --- | --- | --- | --- |
| `damage` | Damage dealt | players, total | fight outcome, also shown by the built-in windows |
| `dps` | Damage per second (per second only) | players, total | fight outcome, also shown by the built-in windows |
| `power` | Strike damage dealt | players, total | fight outcome, also shown by the built-in windows |
| `condi` | Condition damage dealt | players, total | fight outcome, also shown by the built-in windows |
| `down_contrib` | Down contribution: damage that led to enemy downs | players, total | fight outcome, also shown by the built-in windows |
| `downs` | Enemies downed | players, total | fight outcome, also shown by the built-in windows |
| `kills` | Enemies killed | players, total | fight outcome, also shown by the built-in windows |
| `deaths` | Deaths | players, total | fight outcome, also shown by the built-in windows |
| `downed` | Times downed | players, total | fight outcome, also shown by the built-in windows |
| `taken` | Damage taken | players, total | fight outcome, also shown by the built-in windows |
| `cleanses` | Condition stacks cleansed from allies | players, total | fight outcome, also shown by the built-in windows |
| `strips` | Boon stacks stripped from enemies | players, total | fight outcome, also shown by the built-in windows |
| `healing` | Healing given (for others, only what they gave you) | players, total | fight outcome, also shown by the built-in windows |
| `barrier` | Barrier given (for others, only what they gave you) | players, total | fight outcome, also shown by the built-in windows |
| `hps` | Healing and barrier given per second (per second only; for others, only what they gave you) | players, total | fight outcome, also shown by the built-in windows |
| `healing_in` | Healing received (for others, only what you gave them) | players, total | fight outcome, also shown by the built-in windows |
| `barrier_in` | Barrier received (for others, only what you gave them) | players, total | fight outcome, also shown by the built-in windows |

Heal stats for other players are dimmed. The game reports only what passed between them and you.

## Never offered

- Gear and its stats.
- Builds: traits, skills on the bar and templates.
- Cooldowns.
- Anything ArenaNet disallows.
