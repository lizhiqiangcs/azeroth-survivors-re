# 平衡模拟框架

以后所有数值和技能调整都先在这里跑。它在原生环境（不是浏览器）里以单线程高速跑完整的 15 分钟对局，由 AI 玩家做决策，再把结果汇总成报告。

代码在 `crates/benilla-app/src/survivors/sim/`，只在 `cargo test` 下编译，不进游戏包。

## 怎么跑

机器 CPU 负载高会断电，所以一律 2 个编译任务、单线程模拟：

```bash
# 一批对局 + 报告（默认 1000 局，6 类玩家轮流）
SIM_RUNS=3000 CARGO_BUILD_JOBS=2 cargo test -j 2 --profile play -p benilla-app --lib sim_batch -- --ignored --nocapture

# 只看法师、只跑高手和新手
SIM_CLASS=Mage SIM_PROFILES=expert,novice cargo test -j 2 --profile play -p benilla-app --lib sim_batch -- --ignored --nocapture

# 自动调参：每轮把每个技能往本职业中位 DPS 拉、把敌人血量往目标胜率拉
SIM_ROUNDS=6 SIM_TARGET_WIN=0.6 cargo test -j 2 --profile play -p benilla-app --lib sim_tune -- --ignored --nocapture

# 学卡牌价值：只跑随机选卡，因果效应写进 target/sim/values.tsv，高手选卡会读它（改了卡后重跑）
SIM_RUNS=3000 cargo test -j 2 --profile play -p benilla-app --lib sim_learn -- --ignored --nocapture

# 一局逐秒追踪（血量、击杀、DPS、每次选卡）
SIM_PROFILES=expert SIM_CLASS=Mage cargo test -j 2 --profile play -p benilla-app --lib sim_trace -- --ignored --nocapture

# 和真实游戏对照：模拟游戏自带的自动玩家（?survivors_auto=Mage），按分钟打印等级/击杀分位数
SIM_CLASS=Mage SIM_MAP=0 cargo test -j 2 --profile play -p benilla-app --lib sim_calibrate -- --ignored --nocapture
# 同上，但用 ?survivors_auto=Mage:late 场景（全技能满级、从 600 秒开始），构筑固定，只比机制
cargo test -j 2 --profile play -p benilla-app --lib sim_calibrate_late -- --ignored --nocapture

# 试一个改动而不重新编译：写一个调参文件
echo "fireball = 1.2" > /tmp/try.txt
SIM_TUNE=/tmp/try.txt cargo test ... sim_batch ...
```

| 环境变量 | 含义 | 默认 |
|---|---|---|
| `SIM_RUNS` | 局数 | 1000（调参每轮 900） |
| `SIM_PROFILES` | 玩家类型，逗号分隔 | 全部 6 类 |
| `SIM_CLASS` | 只跑某职业（英文名） | 全部 |
| `SIM_MAP` | 只跑某张地图（编号） | 随机 |
| `SIM_SEED` | 随机种子，同种子同结果 | 1 |
| `SIM_LABEL` | 报告目录名 | `latest` |
| `SIM_TUNE` | 调参文件 | 无 |
| `SIM_THREADS` | 线程数，最多 2 | 1 |
| `SIM_LEARNED` | 设为 0 不读学到的卡牌价值 | 读 |
| `SIM_ROUNDS` / `SIM_TARGET_WIN` | 调参轮数 / 高手目标胜率 | 5 / 0.6 |

报告写到 `target/sim/<label>/`：`report.md`（读这个），`runs.csv`、`abilities.csv`、`cards.csv`（拿去表格里筛）。调参结果在 `target/sim/tune/`：`tune.txt`（可直接当 `SIM_TUNE` 用）、`suggested.md`（建议写回 `data.rs` 的新数值）。

## 和游戏同源

模拟器不抄数值，直接调用游戏代码：

- 技能、天赋、天赋树、专精、联动技、物品、附魔、被动、职业/种族特性、敌人、地图、波次、Boss 节奏：`data.rs`、`synergy.rs`、`actives.rs` 原样读取。
- 角色属性、伤害倍率、DoT 倍率、技能冷却/数量/范围：`Run::stats`、`Run::power`、`Owned::*`。
- 升级发牌和选卡后的效果（专精换起手技、学深层技、联动技、重抽、放逐）：`hud::next_offer`、`hud::pick`、`hud::reroll`。
- 受伤（护甲、护盾、无敌、石像形态、怒气）：`hero::hurt_hero`。
- Q/E 主动技能对角色的效果：`Run::activate`；对每个敌人的效果：`actives::touch`。

游戏里由 ECS 系统做的部分（刷怪、追击、施法、命中、状态、触发、掉落）在 `sim/world.rs` 里按同样规则重写，去掉了画面。为了防止改了游戏忘了改模拟，`sim/sync.rs` 有一个测试：扫描游戏系统里所有 `has_rune("…")`、`has_talent("…")`、`has_relic("…")`、`.key == "…"`，只要有一个机制模拟器既没共用也没实现，`cargo test` 就失败并列出来。

所以：**改数值、加技能、加天赋、加物品，直接跑模拟就是新版本**；只有新加了一种“游戏系统里写死的特殊机制”时，才需要在 `world.rs` 补一段，测试会提醒你。

## AI 玩家

| 玩家 | 选卡 | 走位 | Q/E/F |
|---|---|---|---|
| `novice` 新手 | 随机 | 站桩，被围死才挪一步 | 快死才按减伤，偶尔乱按爆发 |
| `casual` 休闲 | 爱拿新技能和升级 | 绕圈风筝 | 有怪就爆发，残血按减伤和位移 |
| `expert` 高手 | 按专精、联动、深层技规划，烂牌会重抽 | 血健康时站桩输出，残血或被围时风筝捡经验 | 爆发留给 Boss 或满场怪；引爆类爆发等足够多敌人带上燃烧/DoT/破甲/审判再按；减伤在真有危险时按；冲锋类位移用来贴脸，其余用来突围 |
| `expert_stand` | 同高手 | 只站桩 | 同高手 |
| `random_picks` | 随机 | 同高手 | 同高手 |
| `expert_nokeys` | 同高手 | 同高手 | 几乎不按 |

最后三类是对照组，用来拆开三种技巧各值多少：

- **选择的价值** = 高手 − 随机选卡（同样走位和操作）的胜率差。
- **走位的价值** = 高手 − 站桩高手。
- **操作的价值** = 高手 − 不按 Q/E/F 的高手。

## 评估指标

报告第一节把体验压成几个 0–100 的分数，每个都有目标区间，改动前后对比一眼能看出变好还是变坏：

| 维度 | 怎么量 | 目标 |
|---|---|---|
| 挑战 | 高手胜率 | 45–75%，最好 60% |
| 爽感 | 每分钟“割草秒”（一秒内杀 ≥4 且血量 >60%）；每局“选卡后 DPS 跃升 ≥30%”的次数；单秒最多击杀 | ≥15 秒/分；≥6 次；≥25 |
| 紧张 | 每局残血（<25%）后脱险（>50%）次数；低于 30% 血的时间占比 | ≥1 次；3–15% |
| 策略 | 选择的价值；“两难手牌”占比（两张好卡分差 ≤8）；可用专精占比（胜率与最强差 15pp 内） | ≥20pp；≥25%；越高越好 |
| 走位价值 | 高手 − 站桩高手 | 10–40pp：走位有用，但站桩流也能活 |
| 操作价值 | 高手 − 不按键高手 | 8–35pp |
| 上手 | 新手中位存活时间；新手胜率 | ≥300 秒；5–35% |
| 挫败（越低越好） | 4 分钟内阵亡率、5 秒内满血暴毙占阵亡比、“无用手牌”占比、超过 2 分钟等不到升级的局占比 | 加权 ≤15 好，≤30 尚可 |

手牌好坏统一用高手的估值来判断（不管实际是谁在选），所以不同玩家的“无用/无脑/两难手牌”可直接比较。

## 平衡数据

- **职业 × 专精**：胜率、存活、等级、每局联动技和深层技数，以及爆发技能的直接伤害占比。偏离 30–85% 的专精会列到最后的“需要关注”。
- **技能强度**：每个技能“持有期间每秒伤害”与本职业中位数之比，>1.6 过强、<0.5 过弱；持有与不持有时的胜率差。
- **伤害来源**：含触发效果（Ignite、震击、雷霆之怒、深伤等），看哪个机制在扛伤害。
- **卡牌**：拿到某张卡后的胜率提升。用随机选卡的对局来算，因为随机选择等于随机实验，提升就是这张卡的因果效应；高手的选择会跟整套规划混在一起。同时给出高手被提供时的选取率：选取率接近 100% 且提升高，就是无脑必选卡。
- **联动技与深层技**：获得率、中位获得时间、获得局胜率。
- **阵亡时间分布**：按分钟，Boss 每 135 秒一个，尖峰落在 Boss 分钟说明 Boss 太硬或伤害跳变。

## 和真实游戏的差别

- 步长 0.1 秒；飞行物的飞行时间不计（同一步结算），对平衡影响很小。
- 平地无地形，敌人用粗网格分开，英雄在原点 40 码范围内活动（游戏里的拴绳）。
- 视觉、声音、相机都没有。
- 自动施法、目标选择、命中状态、死亡传承按游戏代码一一对照实现；有差异时以游戏为准，修 `world.rs`。

## 校准

模拟器对不对，拿游戏自己的自动玩家来量：浏览器开 `?survivors_auto=Mage`（或 `:late`），游戏每 10 秒在控制台打一行 `t= level= kills= damage=`；`sim_calibrate` / `sim_calibrate_late` 用同一套走位（`Movement::Auto`，逐行照抄 `auto::plan`）、同样随机选卡、不按 Q/E/F，打印同样的数。

2026-10-28 的对照（法师，地图 0）：

| 场景 | 真实游戏 | 模拟器中位数 |
|---|---|---|
| `:late` +9 秒 | 伤害 23k，击杀 34，8 级 | 23k，35，10 级 |
| `:late` +21 秒 | 伤害 ~150k，击杀 ~290 | 164k，287 |
| `:late` +31 秒 | 伤害 351k，击杀 572，42 级 | 283k，541，40 级 |
| 普通开局 2 分钟（第 2 局） | 15 级，333 杀 | 15 级，350 杀（四分位 251–423） |
| 普通开局 2 分钟（第 1 局，只拿到寒冰箭） | 7 级，138 杀 | 落在下四分位以下：单局选卡运气 |

固定构筑下机制吻合到 ±20%，随机选卡的单局差异远大于模拟误差。改了伤害结算、敌人或掉落后重跑一次校准。

调参文件还能改敌人成长曲线：`hp_p`/`hp_p2`、`damage_p`/`damage_p2` 代替 `data::hp_scale`/`damage_scale` 里的 `1 + a·p + b·p²`。

## 速度

单线程，每局约 9000 步、同时数百个敌人；实测约 2.6–3.6 万局/小时（见每份报告第一行）。
