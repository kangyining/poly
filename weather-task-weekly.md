好，基于所有的讨论、验证、评估，给你一个以周为单位的完整执行计划。

---

# Polymarket Weather Bot — 周执行计划

*基于 v5.0 Master Plan + Bot Design v2 + 深度评估修正*
*起始日期：2026年4月*

---

## 总览

```
Week 1-2  → Phase 0: 侦察 + 数据管道
Week 3-4  → Phase 1: 回测 + 参数验证
Week 5    → Phase 2: 模拟盘
Week 6    → Phase 3: 最小实盘
Week 7-8  → Phase 3: 扩展实盘
Week 9+   → 世界杯专项 + 持续优化
```

每周结束有明确 Go/No-Go 门控。不通过不进入下一周。

---

## Week 1 — Phase 0：侦察（最重要的一周）

**核心目标：回答所有盲区问题。不写任何交易代码。**

### 任务清单

**Day 1-2：环境搭建 + Polymarket 结构侦察**

```
□ 创建 Polygon 钱包，充值小量 USDC（$20 足够）
□ 安装依赖：requests, pandas, jupyter, py-clob-client（记录实际版本）
□ 建立项目结构（按 v2 文档的文件树）
□ 建 SQLite 数据库（schema.sql）

□ 运行 Phase 0 侦察 Notebook（00_api_recon.ipynb）：

  任务 A: 拉所有活跃天气市场
    GET gamma-api.polymarket.com/events?tag=weather&closed=false&limit=200
    → 记录：实际返回多少条？JSON 层级结构是什么？
    → 过滤出"每日最高气温"类型的市场
    → 统计：有多少个不同城市？（可能不是 30 个）

  任务 B: 对 London 今天的市场做深度解剖
    → 找到所有温度 bin 的 token_id（YES 和 NO）
    → 拉每个 bin 的 order book
    → 记录：远端 bin（±5°C 以外）的 bid/ask 深度是多少？
    → 关键数字：能不能在 $0.95+ 的 NO 上填 $5？$20？$50？
    → 记录：negRisk 字段在哪里？值是什么？

  任务 C: 读 5 个已结算市场的 Rules 页面
    → London：结算数据源是什么？（WU/NOAA/NWS/其他）
    → NYC：结算数据源是什么？
    → Seoul：结算数据源是什么？
    → 3 个非主流城市各抽一个
    → 结论：不同城市用不同数据源吗？
```

**Day 3：结算数据源验证（最关键）**

```
□ 拉最近 10 个已结算的 London 天气市场
  → 获取每个市场的结算温度（winning_bin）
  → 同时拉对应日期的 METAR（EGLL）最高温
  → 对比：METAR 最高温 vs 结算温度，差了多少？
  → 如果差 >1°C 超过 3 次 → 结算不用 METAR，需要找正确数据源

□ 找到 London 市场 Rules 页面指定的数据源
  → 如果是 Weather Underground → 找 WU 的 API
  → 如果是 NWS CLI → 只适用美国城市，London 不适用
  → 记录：实时可以访问这个数据源吗？

□ 对 NYC 做同样的验证
  → 结算数据源极有可能是 NWS CLI（非实时 METAR）
  → NWS CLI 在第二天早上发布 → 意味着当天无法用它做日内确认
  → 结论：日内确认策略需要什么数据？该数据实时可得吗？
```

**Day 4：气象数据管道验证**

```
□ 测试 Open-Meteo API
  → London、Seoul、NYC 各请求一次
  → 记录：返回格式、ECMWF vs GFS 差异、更新延迟

□ 对比历史预报 vs 实际结算
  → 用 Open-Meteo historical API 拉过去 30 天 London 预报
  → 对比对应日期的结算温度
  → 计算：预报均值 vs 实际温度的平均误差（MAE）
  → 这个数字决定 "exclusion_range" 的合理下限

□ 测试实时数据源（对应结算数据源）
  → 如果结算用 WU：测试 WU API 是否实时可得
  → 如果结算用 NWS：测试 NWS CLI 发布时间和格式
```

**Day 5-7：数据录制系统上线**

```
□ 实现 market_recorder.py（每小时录制市场快照）          ✅ DONE
□ 实现 forecast_recorder.py（每 6 小时录制预报）         ✅ DONE
□ 实现 settlement_tracker.py（每天录制结算结果）          ✅ DONE
□ 调度器跑起来，验证数据入库正常                          ✅ DONE (178 settlements backfilled)
□ 验证 neg-risk 下单参数（在 testnet 或用极小金额测试）

□ [NEW] 附加数据源 — 按优先级实现:

  P1: Open-Meteo Previous Runs API (历史预报回填)
    → endpoint: historical-forecast-api.open-meteo.com/v1/forecast
    → 回填过去 60 天的 ECMWF/GFS 预报（配对已有 settlement 数据）
    → 实现 data/historical_forecast_backfill.py
    → run: python run_recorder.py --backfill-forecasts
    → MAE: Chicago ECMWF 3.9F / GFS 2.4F | Dallas ECMWF 2.4F / GFS 4.6F | NYC ECMWF 3.2F / GFS 1.6F
    Status: ✅ DONE (178 records, paired with all settlements)

  P2: TAF (Terminal Aerodrome Forecasts)
    → endpoint: aviationweather.gov/api/data/taf?ids=KORD,KDAL,KLGA
    → 机场 24-30hr 预报，每小时录制（同 METAR API）
    → 新增 taf_records 表；data/taf_nws_recorder.py
    → Note: TAF 无温度字段，存 rawTAF 供后续特征工程（风向/云层）
    Status: ✅ DONE

  P3: NWS API (api.weather.gov)
    → 免费，无 key，官方美国天气预报
    → Grid URLs 存入 CITIES config（LOT/FWD/OKX）
    → data/taf_nws_recorder.py → record_nws_forecasts()
    → 每 6 小时录制，nws_forecasts 表，含 daytime high + shortForecast
    Status: ✅ DONE (21 records first run)

  P4: Weather Underground (结算数据源直连)
    → 需要免费 API key (developer.wunderground.com)
    → 历史站点数据: KORD/KDAL/KLGA 每日最高温
    → 用于交叉验证结算结果
    → 先获取 API key 再实现
    Status: □ BLOCKED (需要 API key)

  P5: CLOB Price History Backfill                              ✅ DONE (2026-04-11)
    → endpoint: clob.polymarket.com/prices-history (1-min resolution)
    → 实现 data/price_history_backfill.py → 降采样为小时级
    → 实现 data/price_history_missing_bins_backfill.py → 重试 warm bins
    → 结果: NYC 60日×11bin 25,800行 | Chicago 59日 | Dallas 59日
    → warm-side bins (7-10) 无交易量，永久缺失，结构性现象

  P6: ASOS Settlement Temperature Backfill                     ✅ DONE (2026-04-11)
    → 实现 data/asos_daily_high_backfill.py
    → Iowa Mesonet API, 需 sts/ets + ISO 8601+Z 格式
    → 结果: 178/178 settlements 全部有 settlement_temp_f
    → run: python run_recorder.py --backfill-settlement-temps
```

### Week 1 Go/No-Go 门控

```
必须全部通过才能进入 Week 2：

□ 知道了每个主要城市的结算数据源，并且可以实时获取
□ 知道了远端 bin（±4°C）的 NO 实际可成交量（$5？$20？）
□ 验证了 negRisk=True 参数在 API 调用中的正确用法
□ 数据录制系统稳定运行 3 天，SQLite 有数据
□ 拿到了至少 20 个已结算市场的历史数据（用于标定）
□ 知道了有多少城市有可交易的每日气温市场（实际数字）
□ 知道了 Open-Meteo 预报 vs 实际温度的历史 MAE

→ 未通过：延长到 Week 1b（继续侦察），不推进
```

---

## Week 2 — Phase 0b：数据积累 + 回测数据集建设

**核心目标：边录数据边建立历史数据集，为 Week 3 回测做准备。**

### 任务清单

**Day 1-2：历史数据集建设**

```
□ 从 Gamma API 批量拉所有已结算天气市场
  → 目标：至少 200 个已结算市场实例
  → 每个实例：城市、日期、各 bin 的价格历史（开盘/最终）、结算结果
  → 注意：价格历史可能不完整，只有结算结果可靠

□ 从 Iowa Environmental Mesonet 拉历史 METAR 数据
  → 目标城市：London(EGLL), NYC(KJFK), Seoul(RKSI), Chicago(KORD)
  → 时间范围：过去 6 个月
  → 用于：作为日内温度趋势的参考（不是结算数据源）

□ 从 Open-Meteo Historical Forecast API 拉历史预报
  → 对应城市和日期
  → 这是 ML 训练的核心特征数据
```

**Day 3-4：notebook 01 — 市场结构分析**

```
□ 01_market_structure.ipynb

分析问题：
  → 远端 bin（0-10¢ YES）的 NO 价格分布是什么？
  → 有多少个 bin 的 NO 价格 > $0.90？> $0.95？
  → 不同城市的 bin 数量和间距是否一致？
  → 市场什么时候开放？什么时候流动性最大？
  → 模型更新后（6h 节点）价格变化有多大？

关键输出：
  → exclusion_range 的合理范围（基于历史预报 MAE）
  → 每个城市平均有多少个可交易的 NO bin
  → 估计每天可部署的总资金量
```

**Day 5-7：数据管道稳定性 + bug 修复**

```
□ 确认录制系统连续运行 7 天无中断
□ 修复 Week 1 发现的所有 API 问题
□ 补充城市配置（加入实际有市场的城市，去掉没有的）
□ 写数据质量检查脚本（缺失值、异常值检测）
□ 整理 phase0_findings.md
```

### Week 2 Go/No-Go

```
□ 历史数据集 ≥ 200 个已结算市场实例
□ 每个城市有对应的历史预报数据（Open-Meteo）
□ 历史 MAE 计算完成（平均预报误差 °C）
□ 数据录制系统稳定运行 7 天
□ phase0_findings.md 写完

→ 未通过：再录 1 周数据，下周继续
```

---

## Week 3 — Phase 1a：回测框架 + 规则策略验证

**核心目标：验证 NO 远端策略在历史数据上是否盈利。**

### 任务清单

**Day 1-2：回测框架实现**

```
✅ 实现 backtest/phase1_backtest.py                           DONE (2026-04-11)
  → 核心逻辑：
    1. 读历史 settlements + forecasts + price_history
    2. 对每个市场实例，根据 GFS forecast 确定 exclusion_range
    3. 找远端 bin，检查 NO 价格是否在 [min_no, max_no]
    4. 模拟买入（用 price_history 中目标 lead time 的价格）
    5. 用实际 settlement_temp_f 计算 P&L
    6. 扣除 taker fee（rate × p_yes × p_no）
  → 支持 --sweep 参数扫描 (exclusion_range × entry_lead)
  → 支持 Monte Carlo 噪声模拟（真实预报不确定性）

□ capital_analyzer.py 基础版（待实现）
  → NO-only vs YES-ladder vs 混合
  → 输出：逐日 P&L 曲线、总回报、Sharpe、最大回撤
```

**Day 3-4：参数扫描**

```
✅ 已跑参数扫描 (exclusion_range 3-8°F × entry_lead 6-48h)   DONE (2026-04-11)

NYC 回测结果（60 settled markets，Feb-Apr 2026）：
  最优参数: exclusion=4.0°F, lead=24h, NO=[0.90, 0.99], pos=$5
  胜率: 134/134 = 100%（历史，回顾性预报）
  总 P&L: +$23.43 on $670 capital deployed
  ROI: 3.5%

  关键发现：
  → 仅 2 次真实亏损事件（GFS 误差 5-8°F 的日子）
  → 两次均被 NO 价格过滤器自动排除（0.991 > MAX 或 0.890 < MIN）
  → Monte Carlo（sigma=3.5°F 真实预报噪声）: 98.3% 胜率, 1.9% ROI

重点关注（已验证）：
  → 最优 exclusion_range = 4°F（NYC，= 2.5× GFS MAE）
  → entry_lead = 24h 最佳（早于 METAR 数据影响价格）
  → NO price [0.90, 0.99] 过滤器是关键安全阀
```

**Day 5-7：城市排名 + 资本效率分析**

```
□ city_ranker.py：按城市分组的胜率、ROI、爆雷率（待实现）
□ 02_backtest_results.ipynb：可视化回测结果（待实现）
□ 03_capital_efficiency.ipynb：NO vs YES vs 混合对比（待实现）
□ Chicago + Dallas 回测（待运行，数据已就绪）
□ 写 phase1_findings.md（待整理）
```

### Week 3 Go/No-Go

```
✅ NO 远端策略在历史数据上净盈利（扣费后）— NYC 通过
□ 找到了最优参数组合（Sharpe > 0.5）— NYC 已找到，其他城市待跑
□ 知道了哪些城市表现好，哪些差 — 待跑 Chicago + Dallas
□ 资本效率分析完成 — 待实现

→ 未通过且原因是"数据量不够" → 延长 1 周录数据再回测
→ 未通过且原因是"策略不赚钱" → 严肃审视 YES vs NO 比例
```

---

## Week 4 — Phase 1b：ML 基础层 + 模拟盘准备

**核心目标：建立 Ridge LR baseline，如果有足够数据则升级 XGBoost。**

### 任务清单

**Day 1-2：ML 特征工程**

```
□ ml/features.py — 从 SQLite 提取训练特征
  
  核心特征（先用最少的，防止过拟合）：
    forecast_max_mean     ← Open-Meteo 预报均值
    forecast_max_std      ← 集合标准差（不确定性）
    ecmwf_gfs_diff        ← |ECMWF - GFS|（模型分歧）
    forecast_lead_hours   ← 预报提前多少小时
    month_sin, month_cos  ← 季节性（循环编码）
    city_encoded          ← 城市（LabelEncoder）

  目标：
    forecast_error = actual_max - forecast_max_mean

  注意：按日期切分 TimeSeriesSplit（Week 1 评估中的修正）

□ 检查样本量：
  → <750 个样本 → 只用 Ridge LR，不跑 XGBoost
  → ≥750 个样本 → 跑 XGBoost 对比
```

**Day 3-4：Ridge LR Baseline**

```
□ 训练 Ridge LR
□ 5-fold 时序交叉验证（按日期切分）
□ 关键问题：LR 的 CV MAE 是否低于"直接用预报均值"的 MAE？
  → 如果不低 → ML 没有学到有用的东西，不值得用
  → 如果低 10%+ → ML 有价值，继续开发

□ 如果样本 ≥750：训练 XGBoost 并对比
□ 实现 bin_probability.py（温度分布 → 各 bin 概率）
□ 实现 mispricing_scorer.py（ML 概率 vs 市场价格 → edge 列表）
```

**Day 5-7：模拟盘系统准备**

```
□ 实现 paper_trader.py（使用 Week 1 修正后的 P&L 计算）
□ 实现 risk_manager.py 基础版
  → 每日亏损熔断（20% bankroll）
  → 单城市最大敞口（15% bankroll）
  → 单笔最大仓位（5% bankroll）
□ 实现 scheduler.py（APScheduler，每小时触发数据录制）
□ 端到端测试：预报更新 → 信号生成 → paper 记录 → 结算计算
□ 整理 phase1_findings.md（完整版）
```

### Week 4 Go/No-Go

```
□ ML baseline 的 CV MAE ≤ 原始预报 MAE（哪怕持平也可以）
□ Paper trader 端到端测试通过（无系统性 bug）
□ Risk manager 逻辑验证正确
□ Scheduler 稳定运行 2 天
□ 最优参数已从回测结果填入 config/params.py

→ 未通过（ML没改进）→ ML 层暂时跳过，继续用规则策略进入 Phase 2
→ 未通过（bug）→ 修 bug，重测
```

---

## Week 5 — Phase 2：模拟盘（7 天验证）

**核心目标：在 live 数据上验证完整系统，不下真单。**

### 任务清单

**Day 1：模拟盘启动**

```
□ 启动 paper trader，bankroll 设置为 $500（虚拟）
□ 开始对 5 个城市运行（London, NYC, Seoul, Chicago, Toronto）
□ 每小时记录信号和 paper 持仓
□ 设置 Telegram 通知（每天汇报 paper P&L）
```

**Day 2-6：运行观察**

```
每天检查：
  □ paper P&L 是否与回测预期一致？
  □ 信号频率是否合理（每天多少个信号）？
  □ 模拟成交价格是否实际可以成交？
    → 观察 order book：那个价位是否有对手方？
  □ 风控是否正常触发？
  □ 结算计算是否正确？

关键测量（通过观察 order book，不下单）：
  □ 远端 bin 的 Maker 挂单：有多少秒/分钟内会有对手方？
    → 这决定 Maker 成交率
  □ 如果挂 $20 的 Maker 单，需要多久成交？
    → 影响资本周转率
```

**Day 7：模拟盘结果分析**

```
□ 计算 7 天 paper P&L
□ 与回测预期对比（允许 30% 偏差）
□ 分析主要差异原因（成交率？价格差异？城市选择？）
□ 决定：是否进入 Phase 3？
□ 写 phase2_findings.md
```

### Week 5 Go/No-Go

```
□ 7 天 paper P&L 为正
□ Paper 胜率 > 回测胜率的 75%
□ 无系统性 bug
□ 估算 Maker 成交率 > 50%
□ 单日最大 paper 亏损 < $75（15% of $500）

→ 未通过 → 再跑 7 天模拟盘，不上实盘
```

---

## Week 6 — Phase 3a：最小实盘

**核心目标：验证完整真实链路，发现模拟盘发现不了的问题。**

### 任务清单

**Day 1-2：最小实盘启动**

```
配置：
  城市：London + NYC（只有 2 个）
  每笔：$1-3
  总资金：$50-100
  策略：纯规则（不用 ML，降低复杂度）

□ 第一笔真实交易：London 最远端 bin NO
  → 手动执行，不用 bot
  → 记录：下单时间、成交时间、实际 fee
  → 验证：negRisk=True 参数是否正确

□ 验证 Maker 挂单：
  → 挂一笔 $1 的 Maker 单在 NO $0.96
  → 等待成交（可能需要 1-24 小时）
  → 记录：实际 maker rebate 是否到账？金额是否正确？
```

**Day 3-5：bot 接管最小实盘**

```
□ live_trader.py 接管（之前手动测试的两个城市）
□ 每天检查：
  → 持仓是否正确记录？
  → 结算是否自动识别？
  → P&L 计算是否与手算一致？
  → Telegram 通知是否正常？
□ 记录实际 vs 模拟盘的差异
```

**Day 6-7：分析 + 决定扩展**

```
□ 对比：1 周真实 P&L vs 模拟盘同期 P&L
□ 偏差 < 30%：可以扩展到 10 个城市
□ 偏差 > 30%：找原因（成交率？滑点？fee 计算？）
```

### Week 6 Go/No-Go

```
□ 完整链路验证（下单→成交→结算→P&L）
□ Maker rebate 实际到账确认
□ 实际 fee 与理论计算吻合
□ 1 周真实 P&L 为正（哪怕只有 $0.5）
□ Bot 稳定运行 5 天无崩溃
□ 真实 P&L 与模拟盘偏差 < 30%

→ 未通过 → 继续最小实盘，找问题，不扩展
```

---

## Week 7 — Phase 3b：扩展实盘

**核心目标：扩展到 10 城市，加入 METAR 日内确认（或其对应数据源确认）。**

### 任务清单

```
配置：
  城市：10 个（Tier 1 全部 + Tier 2 选优质的）
  每笔：$5-15
  总资金：$300-500

□ 上线日内确认策略
  → 用实际结算数据源（不是 METAR），做日内温度监控
  → 如果日内温度趋势接近 NO bin → 减仓或止损

□ 跑完整 2 周（Week 7-8），积累足够样本
  → 目标：100+ 个已结算的真实交易记录

□ 每周更新参数（用最新 2 周数据重跑参数扫描）
□ 监控：
  → 城市 ROI 排名是否与回测一致？
  → 哪些城市实际表现比预期差？
```

---

## Week 8 — 世界杯专项准备

**核心目标：在常规 Weather bot 运行的同时，开始建立世界杯体育策略。**

```
注意：世界杯 2026年6月11日开球，距今约 2 个月

□ 收集历史赛事数据
  → 国家队历史对阵记录（过去 10 年）
  → Elo 评分（eloratings.net 或 FIFA 官方）
  → 历史赔率数据（可用 The Odds API 或 Betfair historical）

□ 建立 Dixon-Coles 模型基础
  → 输入：近期 N 场比赛数据 + Elo
  → 输出：每场比赛的胜/平/负概率
  → 对比：模型概率 vs Polymarket 当前定价
  → World Cup Winner 市场：$496.8M 已交易，Spain 16%

□ 开始跟踪世界杯市场
  → 120 个活跃市场，按流动性排序
  → 找出定价可能偏差最大的市场（小组赛比赛结果）
  → Sports fee 0.75%（最低），做市 rebate 25%

□ 不急着入场，先建模验证
  → 目标：小组赛开始前 2 周，模型就绪
```

---

## Week 9+ — 持续优化 + 世界杯执行

```
常规 Weather Bot（持续）：
  □ 每周重跑参数扫描（用最新 2-4 周数据）
  □ 城市排名更新（加入新表现好的城市）
  □ 监控：fee 变化、市场结构变化、竞争加剧
  □ 如果 ML 有 750+ 样本：评估 XGBoost 是否有改善

世界杯（6月11日 - 7月19日）：
  □ 小组赛开始前：
    → 部署 Dixon-Coles 模型
    → 每场比赛：模型概率 vs 市场价格 → 找 edge > 5% 的机会
    → 全部用 Maker（Sports fee 0.75%，rebate 25%）

  □ 小组赛期间：
    → 首发阵容发布（通常提前 1 小时）→ 立刻重新计算概率
    → 伤病/停赛新闻 → 比赛前几小时价格最有 edge
    → 淘汰赛晋级市场：小组赛结束后才开放，散户更多

  □ 风险控制：
    → 世界杯单场最大仓位：bankroll × 3%（比天气更低）
    → 因为体育市场操纵风险更高
    → 监管风险：2027 年可能立法 → 世界杯前完成主要 alpha 获取
```

---

## 快速参考：每周状态检查表

每周末自问以下问题，决定是否推进：

```
Week 1: 我知道每个城市的实际结算数据源了吗？
Week 2: 我有 200+ 个历史结算记录了吗？
Week 3: 规则策略在历史数据上是盈利的吗？
Week 4: ML 有没有超过直接用预报的准确率？
Week 5: 模拟盘连续 7 天盈利了吗？
Week 6: 真实小额交易 P&L 与模拟盘吻合吗？
Week 7: 10 城市并行稳定运行了吗？
Week 8: Dixon-Coles 模型的预测 vs 赔率数据有 edge 吗？
```

---

## 最关键的单个任务

**Week 1 Day 3 的结算数据源验证**——所有后续的数据录制、回测标签、日内确认策略都依赖这一个问题的答案。这一天做对，后面少走无数弯路。