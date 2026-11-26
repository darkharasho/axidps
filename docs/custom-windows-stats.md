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

## Boons

Each player also carries `boons`: `{"<boon id>": [up, self, group, squad]}`, integer milliseconds cumulative over the fight. Only boons with a nonzero value appear, and the key is left out when there are none. The measures:

- `up`: how long the boon was on the player.
- `self`: how long the player kept it up on themself.
- `group`: how long the player kept it up on subgroup mates, per target, averaged as axilog does (the ms on each other squad player of the same subgroup in this fight, added up and divided by how many there are).
- `squad`: the same over every other squad player in this fight, per target, averaged as axilog does.

The targets are the squad players (with a subgroup) who took part in this fight's combat, other than the player; the local player always counts. With no targets, `group` or `squad` is 0.

Intensity boons (Might, Stability) are measured in stacks: stack-milliseconds. The numbers follow axilog's boon simulator.

Allowed because: fight outcome, how long a boon was up and who applied it.

| Boon | Id | Measures |
| --- | --- | --- |
| Might | `740` | uptime, self, group, squad |
| Fury | `725` | uptime, self, group, squad |
| Quickness | `1187` | uptime, self, group, squad |
| Alacrity | `30328` | uptime, self, group, squad |
| Protection | `717` | uptime, self, group, squad |
| Regeneration | `718` | uptime, self, group, squad |
| Vigor | `726` | uptime, self, group, squad |
| Aegis | `743` | uptime, self, group, squad |
| Stability | `1122` | uptime, self, group, squad |
| Swiftness | `719` | uptime, self, group, squad |
| Resistance | `26980` | uptime, self, group, squad |
| Resolution | `873` | uptime, self, group, squad |
| Superspeed | `5974` | uptime, self, group, squad |

## Never offered

- Gear and its stats.
- Builds: traits, skills on the bar and templates.
- Cooldowns.
- Anything ArenaNet disallows.
