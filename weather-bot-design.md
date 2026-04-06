# Polymarket Weather Bot — 系统性设计 v2

*设计日期：2026年4月*
*策略基础：@chunxiangai NO-first + 排行榜实证数据*
*Phase 0 验证：2026-04-06（Chicago / Dallas / NYC API 实侦）*

---

## 设计哲学

```
不预测温度，排除温度。
不赌单张桌子，开30张桌子。
不等结算，每小时确认。
NO 做地基，YES 做弹性。

但更重要的是：
  不猜参数，用数据验证。
  不写交易代码，先搞清楚数据。
  不上真钱，先跑模拟盘。
```

---

## 总览：四阶段渐进

```
Phase 0 ──→ Phase 1 ──→ Phase 2 ──→ Phase 3
数据侦察     回测验证     模拟盘       实盘
(Week 1)    (Week 2-3)  (Week 3-4)  (Week 5+)

每个 Phase 结束有 Go/No-Go 门控
不通过 → 不进入下一阶段
```

---

## 技术架构（四层）

```
┌─────────────────────────────────────────────────────────┐
│  Layer 5: EXECUTION（执行层）— Phase 2-3 才写            │
│  paper_trader · live_trader · scheduler · alerts         │
├─────────────────────────────────────────────────────────┤
│  Layer 4: STRATEGY（策略层）— Phase 1 回测时写            │
│  no_distant · intraday_confirm · yes_reprice             │
│  risk_manager · position_manager                         │
├─────────────────────────────────────────────────────────┤
│  Layer 3: ML（模型层）— Phase 1 中期写                    │
│  temp_predictor (LR baseline → XGBoost)                  │
│  bin_probability · mispricing_scorer                      │
├─────────────────────────────────────────────────────────┤
│  Layer 2: ANALYSIS（分析层）— Phase 1 写                 │
│  backtest_engine · param_optimizer                        │
│  capital_analyzer · city_ranker                           │
├─────────────────────────────────────────────────────────┤
│  Layer 1: DATA（数据层）— Phase 0 第一天就写              │
│  market_recorder · metar_recorder · forecast_recorder     │
│  settlement_tracker · SQLite storage                     │
└─────────────────────────────────────────────────────────┘

原则：自下而上搭建，Layer 1 稳固后才往上走
ML 层位于 Analysis 和 Strategy 之间：
  Analysis 提供回测数据 → ML 学习规律 → Strategy 消费 ML 输出做决策
```

### 文件结构

```
polymarket-weather-bot/
│
├── data/                          # Layer 1: 数据层
│   ├── market_recorder.py         # 定时录制市场价格 + order book
│   ├── metar_recorder.py          # 定时录制 METAR 实测温度
│   ├── forecast_recorder.py       # 定时录制预报数据
│   ├── settlement_tracker.py      # 追踪结算结果
│   ├── gamma_client.py            # Gamma API 封装
│   ├── clob_client.py             # CLOB API 封装（只读）
│   └── models.py                  # 数据模型 (dataclasses)
│
├── analysis/                      # Layer 2: 分析层
│   ├── backtest_engine.py         # 回测框架
│   ├── param_optimizer.py         # 参数扫描（exclusion range, price bounds...）
│   ├── capital_analyzer.py        # NO vs YES vs 混合 资本效率对比
│   └── city_ranker.py             # 城市定价偏差排名
│
├── ml/                            # Layer 3: ML 模型层
│   ├── features.py                # 特征工程（从 DB 提取训练特征）
│   ├── temp_predictor.py          # 温度预测模型（LR → XGBoost）
│   ├── bin_probability.py         # Bin 概率估计（模型输出 → 概率分布）
│   ├── mispricing_scorer.py       # 定价偏差评分（ML概率 vs 市场价格）
│   ├── model_evaluator.py         # 模型评估 + 回测对比
│   └── models/                    # 序列化模型文件 (.joblib)
│       └── .gitkeep
│
├── strategy/                      # Layer 4: 策略层
│   ├── no_distant.py              # NO 远端策略
│   ├── intraday_confirm.py        # METAR 日内确认（加仓/止损）
│   ├── yes_reprice.py             # YES 重定价（Phase 3 后期）
│   ├── risk_manager.py            # 风控引擎
│   └── position_manager.py        # 仓位管理（SQLite）
│
├── execution/                     # Layer 5: 执行层
│   ├── paper_trader.py            # 模拟盘（不下真单）
│   ├── live_trader.py             # 实盘（py-clob-client）
│   ├── scheduler.py               # APScheduler 调度
│   └── alerts.py                  # Telegram 通知
│
├── notebooks/                     # 探索性分析
│   ├── 00_api_recon.ipynb         # Phase 0: API 侦察
│   ├── 01_market_structure.ipynb  # Phase 0: 市场结构分析
│   ├── 02_backtest_results.ipynb  # Phase 1: 回测结果可视化
│   └── 03_capital_efficiency.ipynb # Phase 1: 资本效率分析
│
├── config/
│   ├── cities.py                  # 城市配置（坐标、ICAO、时区）
│   ├── params.py                  # 策略参数（全部从回测结果填入）
│   └── .env.example               # 环境变量模板
│
├── db/
│   └── data.db                    # SQLite（市场快照 + METAR + 预报 + 结算）
│
├── findings/                      # 每个 Phase 的发现记录
│   ├── phase0_findings.md         # API 侦察发现
│   ├── phase1_findings.md         # 回测发现
│   └── phase2_findings.md         # 模拟盘发现
│
├── requirements.txt
└── README.md
```

---

## Phase 0: 数据侦察 + API 验证（Week 1）

**目标**：回答所有盲区问题，开始录数据。不写任何交易代码。

### 0.1 Polymarket 市场结构侦察

> **⚠️ Phase 0 实侦结论（2026-04-06）— 已更新**

**Gamma API 访问方式（已验证）**：

```python
# ✅ 正确方式：按 slug 拉具体城市的天气市场
GET https://gamma-api.polymarket.com/events?slug=highest-temperature-in-{city}-on-april-{dd}-2026

# ❌ 错误方式：tag 过滤器无效，会被静默忽略
# GET https://gamma-api.polymarket.com/events?tag=weather&closed=false&limit=200
# 原因：Polymarket 没有 "weather" tag，该参数不起作用

# ✅ 天气市场入口页面
# https://polymarket.com/weather
# 包含 40+ 城市的每日最高温市场

# ✅ 查已结算市场（用于回测数据收集）
GET https://gamma-api.polymarket.com/events?slug=highest-temperature-in-{city}-on-april-{dd}-2026&closed=true

# ✅ 拉指定 bin 的 order book（NO token 直接可交易）
GET https://clob.polymarket.com/book?token_id={clobTokenIds[1]}  # [1] = NO token
```

**已确认的市场结构**：

```
bin 格式：2°F 区间，整数华氏度（Fahrenheit）
         示例："38-39°F"、"≤27°F"（开放低端）、"≥46°F"（开放高端）
bin 数量：每城市 11 个
neg_risk：true（互斥，所有 bin 概率之和 = 1）
token：   clobTokenIds[0] = YES token
          clobTokenIds[1] = NO token（直接可买）
```

**已回答的问题**：

```
✅ bin 边界精确定义
    → 整数华氏度精度（市场描述明确："whole degrees Fahrenheit"）
    → 2°F 区间：[38, 39] 表示 38°F 和 39°F 均落入该 bin
    → 开放区间："≤27°F" 包含所有 ≤27 的整数，"≥46°F" 包含所有 ≥46 的整数

✅ 结算机制
    → 结算数据源：Wunderground 历史数据（非 METAR 直接源，但底层相同）
    → 结算时区：UTC，endDate = 目标日期 12:00 UTC（本地时间早上）
    → "最高温"时间窗口：目标日历天全天，Wunderground 数据完成后结算
    → 注意：交易在 12:00 UTC 停止，但结算可能次日才完成（等 Wunderground 数据定稿）
    → 数据定稿后的修订不计入结算（明确声明）

✅ NO token 操作路径（三种方法全部验证，2026-04-06）

    方法 A：直接 CLOB 买 NO token（clobTokenIds[1]）         ← 推荐
      费用：taker = 5% × p_NO × (1-p_NO)（Weather 类别费率 5%）
            远端 NO @ $0.9945：$0.027/百股（0.03%，可忽略）
            中段 NO @ $0.70：$1.05/百股（1.5%，有意义）
            maker：0 费 + 获得对手方 taker 费的 25% 返还
      速度：taker market order 秒级；maker limit order 等对手方（分钟到小时）
      结论：最简、最快、无额外风险 ✅

    方法 B：splitPosition() → YES+NO → CLOB 卖掉 YES           ← 不推荐
      split 费用：无（仅 Polygon gas ≈ $0.001-0.01）
      卖 YES taker 费：5% × p_YES × (1-p_YES) = 与方法 A 完全相同（公式对称）
      总成本：与方法 A 相同 + gas，但多一步链上交易（5-15 秒）
      远端 bin 问题：YES @ $0.005，流动性极薄，市价卖出滑点严重
      结论：成本相同但更慢更复杂，被方法 A 支配 ❌

    方法 C：convertPositions()（neg-risk adapter 合约）          ← 不适用
      作用：将已持有的 NO token 转换为其他结果的 YES token + USDC
      费用：feeBips/10000 作用于 collateral 输出（具体值未知）
      结论：这是持仓再平衡工具，不是买入 NO 的路径，与策略无关 ❌

    费用（完全确认，2026-04-06）：
      来源：Gamma API feeSchedule 字段 + Fees.sol 合约 + 官方文档费率表

      feesEnabled: true（天气市场确认已启用费用）
      feeType: "weather_fees"
      taker fee rate = 5%（feeSchedule.rate = 0.05）
      maker fee = 0%，rebateRate = 0.25（获对手 taker 费的 25% 返还）

      公式：fee = shares × 0.05 × p × (1−p)
      注：makerBaseFee/takerBaseFee = 1000 bps 是合约层面的上限（MAX_FEE_RATE_BIPS）
          不是实际费率；实际费率由 feeSchedule.rate = 0.05 决定

      实际影响（远端 NO bin）：
        NO @ $0.999：fee = 0.05 × 0.999 × 0.001 = 0.005% → 可忽略
        NO @ $0.970：fee = 0.05 × 0.97 × 0.03 ≈ 0.15%
        NO @ $0.700：fee = 0.05 × 0.70 × 0.30 ≈ 1.05%（中段才有意义）
        p=0.50 时最大：fee = 1.25%（每 100 股 $1.25）

      结论：5% 是费率参数，非"5% 交易额"——远端 bin 实际费用极低

✅ 流动性实况（NYC ≤41°F NO bin 实测，2026-04-06）
    → best bid $0.994（$379 USDC 可用）/ best ask $0.999（$1,159 USDC 可用）
    → spread: $0.005
    → $5 ✅ / $20 ✅ / $100 ✅ 均可在最优价格成交
    → 94 bid 档 / 3 ask 档

✅ 市场日历（已观察）
    → 市场在目标日期约 24-36 小时前开放
    → 停止接单：目标日 12:00 UTC（本地时间早上，日最高温尚未出现）
    → 从停止接单到结算：几小时到次日（等 Wunderground 数据）

✅ 实际规模
    → 已确认 40+ 城市有活跃天气市场（polymarket.com/weather）
    → 可覆盖 30 城市的目标可达
```

**⚠️ 关键修正 — 结算气象站**：

```
设计文档原配置 vs 实际 Polymarket 结算站（差异巨大！）

城市      设计文档配置   Polymarket 实际结算   是否匹配
Chicago   KORD          KORD (O'Hare)        ✅ 正确
Dallas    KDFW (DFW)    KDAL (Love Field)    ❌ 错误！
NYC       KJFK (JFK)    KLGA (LaGuardia)     ❌ 错误！

→ Dallas 和 NYC 的 METAR 监控必须使用 KDAL 和 KLGA
→ config/cities.py 已于 Phase 0 修正
→ Wunderground URLs:
    Chicago: https://www.wunderground.com/history/daily/us/il/chicago/KORD
    Dallas:  https://www.wunderground.com/history/daily/us/tx/dallas/KDAL
    NYC:     https://www.wunderground.com/history/daily/us/ny/new-york-city/KLGA
```

### 0.2 气象数据管道验证

> **⚠️ Phase 0 实侦结论（2026-04-06）— 已更新**

**已验证的数据源**：

```python
# ✅ Open-Meteo — 已验证可用，两模型均返回
GET https://api.open-meteo.com/v1/forecast
    ?latitude={lat}&longitude={lon}
    &daily=temperature_2m_max
    &models=ecmwf_ifs025,gfs_seamless
    &forecast_days=3
    &timezone={tz}

# 返回字段：
#   daily.temperature_2m_max_ecmwf_ifs025  → ECMWF 预报最高温（°C）
#   daily.temperature_2m_max_gfs_seamless  → GFS 预报最高温（°C）
# 注意：Polymarket 结算用 °F，需转换：°F = °C × 9/5 + 32

# 三城市 2026-04-07 实测预报（2026-04-06 ~15:10 UTC 拉取）：
#   Chicago:  ECMWF 0.8°C (33.4°F) / GFS 0.4°C (32.7°F) — 模型一致
#   Dallas:   ECMWF 20.6°C (69.1°F) / GFS 23.1°C (73.6°F) — ⚠️ 2.5°C 分歧！
#   NYC:      ECMWF 8.9°C (48°F) / GFS 8.5°C (47.3°F) — 模型一致

# ✅ METAR — 已验证，温度精度为小数
GET https://aviationweather.gov/api/data/metar?ids=KORD,KDAL,KLGA&format=json
# 更新频率：每小时（整点前约 10 分钟）
# 温度精度：小数（如 7.2°C），但 Polymarket 结算取整数 °F

# ⚠️ 重要：监控的 ICAO 站与设计文档不同（见 0.1 修正）
#   Chicago 监控 KORD  ✅
#   Dallas  监控 KDAL  ← 非 KDFW
#   NYC     监控 KLGA  ← 非 KJFK

# ✅ Iowa Environmental Mesonet — KORD 历史数据已验证
GET https://mesonet.agron.iastate.edu/cgi-bin/request/asos.py
    ?station=KORD&data=tmpf&format=onlycomma&report_type=3
    &year1=YYYY&month1=MM&day1=DD&year2=YYYY&month2=MM&day2=DD
# 频率：每小时（:51 分观测）
# 精度：°F 小数
# 可用性：确认可拉多年历史 METAR 数据

# ⚠️ 关键验证（仍需完成）：METAR/Wunderground 实测 vs Polymarket 结算
# Polymarket 用 Wunderground，Wunderground 底层来自 ASOS（与 METAR 同源）
# 但 Dallas/NYC 使用不同气象站，需交叉验证历史结算数据确认一致性
```

### 0.3 数据录制系统

**从 Phase 0 第一天开始录制，为回测积累数据**

```python
# data/models.py — 数据模型
# 注：bin_label 格式为华氏度字符串，如 "38-39°F"、"≤27°F"、"≥46°F"
#     Gamma API slug 格式: highest-temperature-in-{city}-on-april-{dd}-2026

@dataclass
class MarketSnapshot:
    """每小时录制一次"""
    timestamp: datetime          # 录制时间
    city: str
    date: str                    # 目标日期
    event_id: str
    bin_label: str               # "38-39°F"（华氏度，非摄氏度）
    token_id_yes: str            # clobTokenIds[0]
    token_id_no: str             # clobTokenIds[1]（直接可买 NO）
    yes_price: float
    no_price: float
    yes_best_bid: float          # order book 最优买
    yes_best_ask: float          # order book 最优卖
    book_depth_bid: float        # bid 侧总量（USDC）
    book_depth_ask: float        # ask 侧总量（USDC）
    volume_24h: float


@dataclass
class METARRecord:
    """每小时录制一次"""
    timestamp: datetime
    icao: str
    city: str
    temp_c: float
    dewpoint_c: float
    raw_metar: str


@dataclass
class ForecastRecord:
    """每6小时录制一次（GEFS 更新后）"""
    timestamp: datetime
    city: str
    target_date: str
    source: str                  # "open-meteo" | "gefs" | "ecmwf"
    temp_max_mean: float
    temp_max_std: float          # 如果有集合数据
    temp_max_min: float          # 集合最低
    temp_max_max: float          # 集合最高
    raw_json: str                # 原始返回，备查


@dataclass
class SettlementRecord:
    """每天录制（市场结算后）"""
    city: str
    date: str
    event_id: str
    winning_bin: str             # 哪个 bin 赢了，格式如 "38-39°F"
    settlement_temp: float       # 结算温度（整数华氏度，Wunderground 数据）
    settlement_source: str       # 固定为 "wunderground"
    wunderground_station: str    # 结算气象站（如 KDAL，非 KDFW）
```

```python
# data/market_recorder.py — 核心录制逻辑

"""
调度：
  - 每 1 小时：录制所有活跃天气市场的价格快照
  - 每 1 小时：录制所有城市的 METAR
  - 每 6 小时：录制预报更新
  - 每天 1 次：检查已结算市场，录制结算结果

存储：SQLite（db/data.db）
  - 表：market_snapshots
  - 表：metar_records
  - 表：forecast_records
  - 表：settlement_records
"""

class DataRecorder:

    def __init__(self, db_path: str = "db/data.db"):
        self.db = sqlite3.connect(db_path)
        self._create_tables()

    def record_market_snapshots(self):
        """每小时调用：录制所有活跃天气市场"""
        # 注：不能用 ?tag=weather（无效），改用按城市 slug 拉取
        # slug 格式: highest-temperature-in-{city}-on-april-{dd}-2026
        events = self._fetch_gamma_weather_events()
        for event in events:
            for market in event["markets"]:
                for token in market["tokens"]:
                    book = self._fetch_order_book(token["token_id"])
                    snapshot = MarketSnapshot(
                        timestamp=datetime.utcnow(),
                        city=self._parse_city(event),
                        # ... 填充所有字段
                    )
                    self._insert_snapshot(snapshot)

    def record_metars(self):
        """每小时调用：录制所有城市 METAR"""
        icao_list = [c["icao"] for c in CITIES.values()]
        # 批量请求 aviationweather.gov
        # 注意：Dallas 用 KDAL（Love Field），NYC 用 KLGA（LaGuardia）
        # 与 Polymarket Wunderground 结算站一致
        # ...

    def record_forecasts(self):
        """每6小时调用：录制预报"""
        for city, cfg in CITIES.items():
            forecast = self._fetch_openmeteo(cfg["lat"], cfg["lon"])
            # ...

    def record_settlements(self):
        """每天调用：检查已结算市场"""
        closed = self._fetch_gamma_closed_weather()
        # ...
```

### 0.4 SQLite Schema

```sql
-- db/schema.sql

CREATE TABLE IF NOT EXISTS market_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL,           -- ISO 8601
    city TEXT NOT NULL,
    target_date TEXT NOT NULL,         -- YYYY-MM-DD
    event_id TEXT,
    bin_label TEXT NOT NULL,           -- "22°C"
    token_id_yes TEXT,
    token_id_no TEXT,
    yes_price REAL,
    no_price REAL,
    yes_best_bid REAL,
    yes_best_ask REAL,
    book_depth_bid REAL,              -- USDC
    book_depth_ask REAL,
    volume_24h REAL
);

CREATE TABLE IF NOT EXISTS metar_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL,
    icao TEXT NOT NULL,
    city TEXT NOT NULL,
    temp_c REAL,
    dewpoint_c REAL,
    raw_metar TEXT
);

CREATE TABLE IF NOT EXISTS forecast_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL,
    city TEXT NOT NULL,
    target_date TEXT NOT NULL,
    source TEXT NOT NULL,
    temp_max_mean REAL,
    temp_max_std REAL,
    temp_max_min REAL,
    temp_max_max REAL,
    raw_json TEXT
);

CREATE TABLE IF NOT EXISTS settlement_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    city TEXT NOT NULL,
    date TEXT NOT NULL,
    event_id TEXT,
    winning_bin TEXT,
    settlement_temp REAL,
    settlement_source TEXT
);

-- 索引
CREATE INDEX idx_snapshots_city_date ON market_snapshots(city, target_date);
CREATE INDEX idx_metar_city ON metar_records(city, timestamp);
CREATE INDEX idx_forecast_city_date ON forecast_records(city, target_date);
CREATE INDEX idx_settlement_city_date ON settlement_records(city, date);
```

### Phase 0 Go/No-Go 门控

```
必须全部通过才能进入 Phase 1：

✅ 搞清楚了 bin 结构和结算规则
     → 2°F bins，整数华氏度，Wunderground 数据源
     → endDate = 目标日 12:00 UTC（交易截止），结算可次日完成

✅ 确认了 NO token 买入路径和成本
     → 直接买 clobTokenIds[1]（NO token），无需 split 或 convert

✅ 远端 bin 有足够流动性（>$5/bin）
     → NYC ≤41°F NO 实测：$100 可在最优价格成交

✅ 至少 10 个城市有活跃天气市场
     → polymarket.com/weather 确认 40+ 城市

✅ METAR 数据源和结算数据源一致（或已知差异）
     → 已知差异已修正：Dallas → KDAL，NYC → KLGA
     → Wunderground 底层与 ASOS/METAR 同源，精度对齐待历史验证

❌ 数据录制系统稳定运行 >3 天        ← 尚未开始
❌ 已拉到至少 50 个已结算市场的历史数据  ← 尚未开始

→ 通过：进入 Phase 1
→ 不通过：继续侦察 + 录数据，直到通过
```

---

## Phase 1: 历史回测（Week 2-3）

**前置**：Phase 0 通过 + 积累了足够的历史数据

### 1.1 回测数据集

```
数据来源（按优先级）：
  1. Gamma API 已结算市场 → 结算价格 + 结算结果
  2. Phase 0 录制的市场快照 → 实时价格变化
  3. Iowa Environmental Mesonet → 历史 METAR（多年数据）
  4. Open-Meteo historical forecast API → 历史预报

理想数据集：
  - 30+ 城市 × 30+ 天 = 900+ 市场实例
  - 每个市场：开盘价、结算前价格、结算结果
  - 对应的预报数据和 METAR 实测

最小数据集（如果历史数据不够）：
  - 10 城市 × 14 天 = 140 市场实例（Phase 0 录制的 2 周）
  - 可能不够做统计显著的结论
  - → 继续录制到 4 周再回测
```

### 1.2 回测框架

```python
# analysis/backtest_engine.py

@dataclass
class BacktestConfig:
    # 策略参数（全部可调，用参数扫描找最优）
    exclusion_range: float         # ±N°C 排除范围
    min_no_price: float            # NO 最低买入价
    max_no_price: float            # NO 最高买入价
    position_size: float           # 每个 bin 的仓位（USDC）
    max_city_exposure: float       # 单城市最大敞口
    use_metar_confirm: bool        # 是否启用日内确认
    metar_add_threshold: float     # 确认安全的温度距离
    metar_exit_threshold: float    # 危险区温度距离


@dataclass
class BacktestResult:
    total_trades: int
    winning_trades: int
    losing_trades: int
    win_rate: float
    total_pnl: float
    total_invested: float          # 总锁定资金
    roi: float                     # pnl / invested
    capital_efficiency: float      # pnl / (invested × days_locked)
    max_drawdown: float
    sharpe_ratio: float
    blowup_count: int              # 远端 bin 命中次数
    blowup_cities: list[str]       # 哪些城市爆过
    pnl_by_city: dict[str, float]
    pnl_curve: list[float]         # 逐日累计 P&L


class BacktestEngine:

    def run(self, config: BacktestConfig,
            dataset: BacktestDataset) -> BacktestResult:
        """
        模拟 NO 远端策略在历史数据上的表现

        对每个市场实例：
          1. 读取预报 → 确定排除区间
          2. 找远端 bin → 检查 NO 价格是否在 [min, max] 范围内
          3. 模拟买入（用开盘价或录制的快照价）
          4. 如果 use_metar_confirm：模拟日内 METAR 调仓
          5. 用结算结果计算 P&L
          6. 扣除费用（已完全确认）
             taker: fee = shares × 0.05 × p × (1-p)
             maker: 0 费（+ 25% rebate on taker fees）
             远端 NO @ $0.97+：fee < 0.15%，可忽略
             中段 NO @ $0.70：fee ≈ 1.05%，需计入回测
             注：5% 是费率参数，非 5% 交易额；远端 bin 实际费用极低
        """
        pass
```

### 1.3 参数扫描

```python
# analysis/param_optimizer.py

"""
扫描的参数空间：

exclusion_range:   [2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0] °C
min_no_price:      [0.80, 0.85, 0.90, 0.93, 0.95]
max_no_price:      [0.97, 0.98, 0.99]
position_size:     [1, 2, 5, 10, 20] USDC
use_metar_confirm: [True, False]

总组合数：7 × 5 × 3 × 5 × 2 = 1,050 种配置
每种配置跑一遍回测 → 找 Sharpe 最高 + 回撤最小的
"""

def param_sweep(dataset: BacktestDataset) -> pd.DataFrame:
    results = []
    for exclusion in [2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0]:
        for min_no in [0.80, 0.85, 0.90, 0.93, 0.95]:
            for max_no in [0.97, 0.98, 0.99]:
                for size in [1, 2, 5, 10, 20]:
                    for metar in [True, False]:
                        config = BacktestConfig(
                            exclusion_range=exclusion,
                            min_no_price=min_no,
                            max_no_price=max_no,
                            position_size=size,
                            use_metar_confirm=metar,
                            # ...
                        )
                        result = BacktestEngine().run(config, dataset)
                        results.append({
                            "exclusion": exclusion,
                            "min_no": min_no,
                            "max_no": max_no,
                            "size": size,
                            "metar": metar,
                            "pnl": result.total_pnl,
                            "roi": result.roi,
                            "sharpe": result.sharpe_ratio,
                            "max_dd": result.max_drawdown,
                            "win_rate": result.win_rate,
                            "blowups": result.blowup_count,
                        })
    return pd.DataFrame(results)
```

### 1.4 资本效率对比（关键分析）

```python
# analysis/capital_analyzer.py

"""
核心问题：$500 本金，NO-only vs YES-ladder vs 混合，哪个赚更多？

NO 策略：
  买 NO @ $0.96 → 锁定 $0.96/份 → 赢赚 $0.04/份
  $500 能买 ~520 份 NO → 分散到 30 城市 × ~17 份/城市
  每天利润（如果全赢）：520 × $0.04 = $20.80
  但资金全部被锁定 24-48 小时

YES 策略：
  买 YES @ $0.05 → 锁定 $0.05/份 → 赢赚 $0.95/份
  $500 能买 10,000 份 YES → 但不能全买同一个 bin
  每天用 $50 做 ladder（10 城市 × $5/城市）→ 剩余 $450 空闲
  如果赢 1/5：赚 $95 - 成本 $50 = $45
  但胜率取决于预报精度

混合策略：
  $400 做 NO 远端（稳定底层）
  $100 做 YES ladder（高弹性）
  → 可能是最优解？

必须用回测数据验证，不能靠纸上计算。
"""

def compare_strategies(dataset: BacktestDataset,
                        bankroll: float = 500.0) -> dict:
    """
    在相同数据集上对比三种策略，
    输出：逐日 P&L 曲线、Sharpe、最大回撤、资本利用率
    """
    pass
```

### 1.5 城市排名

```python
# analysis/city_ranker.py

"""
不是每个城市的 edge 都一样。

可能影响 edge 的因素：
  - 预报准确率（海洋性气候 > 大陆性气候？Wellington vs Dallas？）
  - 市场定价效率（流动性高的城市 → 更高效 → 更少 edge？）
  - 散户参与度（美国城市散户更多 → 更多 mispricing？）
  - 时区（亚洲城市在美国睡觉时交易 → 定价可能更低效？）

需要回测验证：
  按城市分组的胜率、ROI、爆雷率
  → 排名 → 优先交易 top 城市
"""
```

### 1.6 ML 模型层（LR baseline → XGBoost）

**核心思路**：用 ML 替代"硬编码排除区间"，让数据自动学习哪些 bin 安全、哪些危险。

#### 为什么需要 ML？

```
当前规则策略的局限：
  - exclusion_range 是固定值（±3°C），但不同城市、不同季节预报误差差很大
  - 伦敦海洋性气候 ±2°C 就够了，达拉斯大陆性气候可能需要 ±5°C
  - 规则不能捕捉"今天预报不确定性特别大"这种动态信息

ML 能做什么：
  - 用历史数据学习：给定 [预报值, 预报源, 城市, 季节, 预报提前天数...]
    → 实际最高温落在什么分布里？
  - 输出的是概率分布，不是点估计 → 直接映射到 bin 概率
  - 自动学习城市级别的预报偏差和不确定性
```

#### Step 1: 特征工程

```python
# ml/features.py

"""
从 SQLite 提取训练特征。每一条样本对应一个 (城市, 日期) 的市场实例。

特征分三组：
"""

def build_feature_matrix(db_path: str) -> pd.DataFrame:
    """
    返回 DataFrame，每行一个 (city, date) 样本

    ── 预报特征（最重要）──
    forecast_max_mean        # Open-Meteo 集合均值
    forecast_max_std         # Open-Meteo 集合标准差（不确定性指标）
    forecast_max_min         # 集合最低预报
    forecast_max_max         # 集合最高预报
    forecast_spread          # max - min（集合发散度）
    forecast_ecmwf           # ECMWF 单独预报
    forecast_gfs             # GFS 单独预报
    ecmwf_gfs_diff           # |ECMWF - GFS|（模型分歧 → 不确定性高）
    forecast_lead_hours      # 预报提前多少小时（越远越不准）

    ── 气象特征 ──
    city_lat                 # 纬度（高纬度 → 更大日变化？）
    city_lon                 # 经度
    month                    # 月份（季节性）
    day_of_year              # 年日（更细粒度季节性）
    is_coastal               # 是否沿海（海洋性气候更稳定）
    recent_temp_volatility   # 过去 7 天该城市实测温度标准差

    ── METAR 日内特征（用于日内模型，非开盘模型）──
    metar_current_temp       # 当前实测温度
    metar_current_high       # 今日到目前为止的最高温
    hours_remaining          # 距结算还有几小时
    temp_trend_3h            # 过去 3 小时温度趋势（上升/下降速率）

    ── 目标变量 ──
    actual_max_temp          # 实际最高温（结算温度）
    winning_bin              # 赢了哪个 bin
    """
    pass
```

#### Step 2: Linear Regression（Baseline）

```python
# ml/temp_predictor.py

from sklearn.linear_model import LinearRegression, Ridge
from sklearn.model_selection import TimeSeriesSplit
from sklearn.metrics import mean_absolute_error, mean_squared_error
import joblib

class TempPredictor:
    """
    温度预测器。先用 LR 建立 baseline，再升级 XGBoost。

    设计原则：
      - 预测的不是"温度是多少"，而是"预报误差是多少"
      - target = actual_max - forecast_mean
      - 这样 LR 只需要学习"预报在什么条件下偏高/偏低"
    """

    def __init__(self, model_type: str = "lr"):
        self.model_type = model_type
        self.model = None
        self.feature_cols = None

    # ── 训练 ──

    def train_lr_baseline(self, df: pd.DataFrame):
        """
        Step 1: Linear Regression baseline

        target: forecast_error = actual_max - forecast_max_mean

        特征（先用最少的）：
          - forecast_max_mean
          - forecast_max_std
          - ecmwf_gfs_diff
          - forecast_lead_hours
          - month (one-hot 或 sin/cos 编码)
          - city (one-hot)

        用 TimeSeriesSplit 交叉验证（不能用随机 split，会数据泄漏）
        """
        feature_cols = [
            "forecast_max_mean", "forecast_max_std",
            "ecmwf_gfs_diff", "forecast_lead_hours",
            "month_sin", "month_cos",  # 季节性循环编码
        ]
        # One-hot 城市（如果城市间差异显著）
        city_dummies = pd.get_dummies(df["city"], prefix="city")
        X = pd.concat([df[feature_cols], city_dummies], axis=1)
        y = df["actual_max_temp"] - df["forecast_max_mean"]  # 预报误差

        self.feature_cols = X.columns.tolist()

        # 时间序列交叉验证
        tscv = TimeSeriesSplit(n_splits=5)
        mae_scores = []
        for train_idx, val_idx in tscv.split(X):
            X_train, X_val = X.iloc[train_idx], X.iloc[val_idx]
            y_train, y_val = y.iloc[train_idx], y.iloc[val_idx]

            model = Ridge(alpha=1.0)  # L2 正则化防过拟合
            model.fit(X_train, y_train)

            y_pred = model.predict(X_val)
            mae_scores.append(mean_absolute_error(y_val, y_pred))

        self.model = Ridge(alpha=1.0)
        self.model.fit(X, y)

        return {
            "model": "Ridge LR",
            "cv_mae_mean": np.mean(mae_scores),
            "cv_mae_std": np.std(mae_scores),
            "feature_importance": dict(zip(self.feature_cols,
                                           self.model.coef_)),
        }

    # ── 升级：XGBoost ──

    def train_xgboost(self, df: pd.DataFrame):
        """
        Step 2: XGBoost（有足够数据后升级）

        优势：
          - 自动处理非线性关系（例如温度极端值的预报偏差更大）
          - 自动处理特征交互（例如 Dallas × 夏天 → 预报偏差大）
          - 内置正则化，不太容易过拟合
          - feature_importance 可解释

        最低数据要求：建议 500+ 样本（~15 城市 × 30 天）
        """
        import xgboost as xgb

        feature_cols = [
            "forecast_max_mean", "forecast_max_std",
            "forecast_spread", "ecmwf_gfs_diff",
            "forecast_lead_hours",
            "city_lat", "city_lon",
            "month_sin", "month_cos", "day_of_year",
            "recent_temp_volatility",
        ]
        # XGBoost 原生支持 categorical，不需要 one-hot
        X = df[feature_cols].copy()
        y = df["actual_max_temp"] - df["forecast_max_mean"]

        self.feature_cols = feature_cols

        tscv = TimeSeriesSplit(n_splits=5)
        mae_scores = []
        models = []

        for train_idx, val_idx in tscv.split(X):
            X_train, X_val = X.iloc[train_idx], X.iloc[val_idx]
            y_train, y_val = y.iloc[train_idx], y.iloc[val_idx]

            model = xgb.XGBRegressor(
                n_estimators=200,
                max_depth=4,           # 浅树防过拟合
                learning_rate=0.05,
                subsample=0.8,
                colsample_bytree=0.8,
                reg_alpha=0.1,         # L1
                reg_lambda=1.0,        # L2
                early_stopping_rounds=20,
            )
            model.fit(
                X_train, y_train,
                eval_set=[(X_val, y_val)],
                verbose=False,
            )

            y_pred = model.predict(X_val)
            mae_scores.append(mean_absolute_error(y_val, y_pred))
            models.append(model)

        # 最终模型用全量数据训练
        self.model = xgb.XGBRegressor(
            n_estimators=200, max_depth=4, learning_rate=0.05,
            subsample=0.8, colsample_bytree=0.8,
        )
        self.model.fit(X, y)

        return {
            "model": "XGBoost",
            "cv_mae_mean": np.mean(mae_scores),
            "cv_mae_std": np.std(mae_scores),
            "feature_importance": dict(zip(
                feature_cols,
                self.model.feature_importances_
            )),
        }

    # ── 预测 ──

    def predict_error(self, features: dict) -> tuple[float, float]:
        """
        输入当前特征 → 输出预报误差的点估计

        返回: (predicted_error, uncertainty_estimate)
          - predicted_error: 预报误差（actual - forecast）
          - uncertainty_estimate: 预测不确定性
            LR: 用残差标准差
            XGBoost: 用 quantile regression 或 CV fold 方差
        """
        pass

    def predict_temp_distribution(self, features: dict,
                                  forecast_mean: float) -> dict:
        """
        输出温度概率分布（核心输出）

        返回: {"mean": 24.3, "std": 1.8, "p05": 21.2, "p95": 27.4}

        方法：
          LR:      adjusted_mean ± residual_std（假设高斯）
          XGBoost: 用 quantile regression 输出 p10/p50/p90
        """
        pass

    # ── 持久化 ──

    def save(self, path: str = "ml/models/temp_predictor.joblib"):
        joblib.dump({"model": self.model, "features": self.feature_cols}, path)

    def load(self, path: str = "ml/models/temp_predictor.joblib"):
        data = joblib.load(path)
        self.model = data["model"]
        self.feature_cols = data["features"]
```

#### Step 3: 从温度分布 → Bin 概率

```python
# ml/bin_probability.py

from scipy.stats import norm

class BinProbabilityEstimator:
    """
    把 ML 预测的温度分布 → 映射到每个 bin 的概率

    这是连接 ML 和 Strategy 的桥梁：
      ML 输出: "明天伦敦最高温 N(22.3, 1.8)"
      Bin 概率: {"20°C": 0.05, "21°C": 0.12, "22°C": 0.28, ...}
      策略输入: "22°C bin 真实概率 28%，市场 YES 价 $0.35 → 高估 7%"
    """

    def estimate_bin_probs(self, temp_dist: dict,
                           bin_edges: list[float]) -> dict[str, float]:
        """
        temp_dist: {"mean": 22.3, "std": 1.8}
        bin_edges: [18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28]

        对每个 bin [low, high)，计算 P(low <= temp < high)
        使用高斯 CDF（LR 阶段）或直接用 XGBoost quantile 输出
        """
        mean, std = temp_dist["mean"], temp_dist["std"]
        probs = {}
        for i in range(len(bin_edges) - 1):
            low, high = bin_edges[i], bin_edges[i + 1]
            p = norm.cdf(high, mean, std) - norm.cdf(low, mean, std)
            probs[f"{low}°C"] = round(p, 4)

        # 开放区间：最低/最高 bin
        probs[f"<{bin_edges[0]}°C"] = round(
            norm.cdf(bin_edges[0], mean, std), 4)
        probs[f">{bin_edges[-1]}°C"] = round(
            1 - norm.cdf(bin_edges[-1], mean, std), 4)

        return probs
```

#### Step 4: 定价偏差评分

```python
# ml/mispricing_scorer.py

class MispricingScorer:
    """
    对比 ML 概率 vs 市场价格 → 找交易机会

    edge = ml_prob - market_price（对 NO 来说反过来）

    例子：
      ML 说 "18°C" bin 概率 = 2%  → NO 真实价值 = $0.98
      市场 NO 价格 = $0.94        → edge = $0.04（4%）
      → 这个 bin 值得买 NO

    这比固定排除区间更精确：
      - 不是"排除 ±3°C 内所有 bin"
      - 而是"这个 bin 的 ML 概率 < 3%，市场价格暗示 6%，有 edge"
    """

    def score_all_bins(self, ml_probs: dict[str, float],
                       market_prices: dict[str, dict]) -> list[dict]:
        """
        ml_probs:      {"18°C": 0.02, "19°C": 0.05, ...}
        market_prices: {"18°C": {"yes": 0.06, "no": 0.94}, ...}

        返回按 edge 排序的交易机会列表
        """
        opportunities = []
        for bin_label, ml_prob in ml_probs.items():
            if bin_label not in market_prices:
                continue
            mkt = market_prices[bin_label]

            # NO edge: ML 说概率低 → NO 应该值更多
            no_fair = 1.0 - ml_prob
            no_market = mkt["no"]
            no_edge = no_fair - no_market

            # YES edge: ML 说概率高 → YES 应该值更多
            yes_fair = ml_prob
            yes_market = mkt["yes"]
            yes_edge = yes_fair - yes_market

            if no_edge > 0.01:  # 至少 1% edge 才考虑
                opportunities.append({
                    "bin": bin_label,
                    "side": "NO",
                    "ml_prob": ml_prob,
                    "fair_price": no_fair,
                    "market_price": no_market,
                    "edge": no_edge,
                    "kelly_fraction": no_edge / (1 - no_market),  # 简化 Kelly
                })
            if yes_edge > 0.02:  # YES 需要更大 edge（波动大）
                opportunities.append({
                    "bin": bin_label,
                    "side": "YES",
                    "ml_prob": ml_prob,
                    "fair_price": yes_fair,
                    "market_price": yes_market,
                    "edge": yes_edge,
                    "kelly_fraction": yes_edge / (1 - yes_market),
                })

        return sorted(opportunities, key=lambda x: x["edge"], reverse=True)
```

#### ML 模型演进路线

```
Phase 1 前期（数据 < 200 样本）：
  → 只用 Ridge LR
  → 特征：forecast_mean, forecast_std, ecmwf_gfs_diff, month, city
  → 目标：学习 forecast_error = actual - forecast
  → 评估：CV MAE < 原始预报 MAE？LR 有没有改善？

Phase 1 中期（数据 200-500 样本）：
  → 加入 XGBoost，和 LR 对比
  → 加入更多特征：lat/lon, volatility, day_of_year
  → 如果 XGBoost CV MAE 比 LR 低 10%+ → 切换到 XGBoost

Phase 2（模拟盘）：
  → 用 ML 概率替代固定排除区间
  → 对比：规则策略 P&L vs ML 策略 P&L
  → 如果 ML 没优势 → 退回规则（ML 只作为辅助信号）

Phase 3（实盘）：
  → 每周用最新数据重训练
  → 监控 ML 预测漂移（prediction drift）
  → 如果连续 3 天 MAE > 阈值 → 告警 + 退回规则策略
```

#### ML Go/No-Go 判断标准

```
ML 值得用的条件（任一满足即可）：
  ✅ LR/XGBoost 的 MAE < 直接用预报均值的 MAE（即 ML 能修正预报偏差）
  ✅ ML 概率 + mispricing scorer 在回测中比固定排除区间多赚 > 10%
  ✅ ML 能识别出规则策略忽略的高 edge 机会（如高不确定性天的 YES 机会）

ML 不值得用的条件（回退到规则）：
  ❌ 样本量 < 100 时 XGBoost 过拟合（CV 和 train MAE 差距 > 50%）
  ❌ LR MAE ≈ 直接用预报均值的 MAE（ML 没学到有用的东西）
  ❌ ML 策略在模拟盘的表现不优于规则策略
```

### Phase 1 Go/No-Go 门控

```
必须全部通过才能进入 Phase 2：

✅/❌ NO 远端策略在历史数据上净盈利（扣费后）
✅/❌ 最优参数组合的 Sharpe > 1.0
✅/❌ 最大单日回撤 < 15% of bankroll
✅/❌ 爆雷（远端命中）频率与预期一致（<5%/城市/天）
✅/❌ 资本效率分析完成，确定 NO/YES/混合 的最优比例
✅/❌ 至少 5 个城市有正 ROI
✅/❌ $500 本金的月预期收益 > $50（10% 月回报）

→ 通过：用最优参数进入 Phase 2
→ 不通过：
   → 如果数据量不够 → 继续录制 2 周再回测
   → 如果策略不赚钱 → 重新审视策略逻辑
   → 如果资本效率太低 → 调整 NO/YES 比例或提高本金
```

---

## Phase 2: 模拟盘（Week 3-4）

**前置**：Phase 1 回测确认盈利 + 确定了最优参数

### 2.1 Paper Trading 系统

```python
# execution/paper_trader.py

"""
接 Live API，生成交易信号，但不下真单。

每次信号生成时：
  1. 记录"如果下单"的价格和数量
  2. 用下一个价格快照模拟成交（假设 Maker 挂单，延迟 1 小时成交）
  3. 用实际结算结果计算 paper P&L

这样验证的是：
  - 策略逻辑在 live 数据上是否正常
  - 信号频率是否合理
  - 风控触发是否正确
  - METAR 日内调仓的实际效果
"""

class PaperTrader:

    def __init__(self, config: BacktestConfig, bankroll: float):
        self.config = config
        self.bankroll = bankroll
        self.paper_positions = []
        self.paper_pnl = 0.0

    def on_forecast_update(self, city: str, forecast: dict, markets: list):
        """预报更新时：生成 NO 远端信号"""
        signals = no_distant_strategy(forecast, markets, self.config)
        for signal in signals:
            if self.risk_check(signal):
                self.paper_positions.append({
                    "city": city,
                    "bin": signal.bin_label,
                    "side": "NO",
                    "price": signal.no_price,
                    "size": signal.size_usdc,
                    "timestamp": datetime.utcnow(),
                    "status": "open"
                })
                self.log(f"[PAPER] BUY NO {city} {signal.bin_label} "
                        f"@ ${signal.no_price:.4f} size=${signal.size_usdc:.2f}")

    def on_metar_update(self, city: str, metar: dict):
        """METAR 更新时：确认/止损"""
        # Layer 2 逻辑
        pass

    def on_settlement(self, city: str, date: str, winning_bin: str):
        """结算时：计算 P&L"""
        for pos in self.paper_positions:
            if pos["city"] == city and pos["status"] == "open":
                if pos["bin"] == winning_bin:
                    # 爆雷：这个 bin 命中了，NO 亏钱
                    pnl = -pos["price"] * (pos["size"] / pos["price"])
                else:
                    # 安全：NO 赢了
                    pnl = (1.0 - pos["price"]) * (pos["size"] / pos["price"])

                pos["status"] = "settled"
                pos["pnl"] = pnl
                self.paper_pnl += pnl
```

### 2.2 执行层验证（不下单但测量）

```
需要测量的指标（通过观察 order book 变化）：

□ Maker 挂单预计成交时间
    → 在远端 bin 挂一个 Maker 价格的 limit order（不实际下）
    → 观察 book 上有没有对手方在这个价位
    → 估计：挂 $5 的单大概多久能成交

□ 滑点估计
    → 当前 best ask = $0.96，如果我要买 $20
    → book 上 $0.96 有 $10，$0.97 有 $15
    → 实际均价 = ($10×0.96 + $10×0.97) / $20 = $0.965
    → 滑点 = 0.005

□ 每日可交易量
    → 30 城市 × 每城市几个 bin 有流动性
    → 总共能部署多少 USDC
```

### 2.3 模拟盘 Go/No-Go 门控

```
必须全部通过才能上实盘：

✅/❌ 连续 7 天 paper P&L 为正
✅/❌ Paper 胜率 > 回测胜率的 80%
✅/❌ 单日最大 paper 亏损 < bankroll × 15%
✅/❌ 预估 Maker 成交率 > 60%（远端 bin 有足够对手方）
✅/❌ 无系统性 bug（仓位追踪、结算计算、信号生成）
✅/❌ METAR 日内调仓逻辑在 live 数据上行为正确
✅/❌ 调度系统稳定运行 7 天无中断

→ 通过：进入 Phase 3 最小实盘
→ 不通过：修复问题 → 再跑 7 天模拟盘
```

---

## Phase 3: 实盘（Week 5+）

### 3.1 最小实盘（Week 5）

```
规模：3 城市 × 每笔 $1-3
资金：$50-100
目标：验证完整链路

验证清单：
  □ 下单 → 成交 → 查询持仓 → 结算 → P&L 全链路
  □ Maker limit order 实际成交时间
  □ Maker rebate 到账确认（每天结算）
  □ 实际滑点 vs 模拟盘估计
  □ 手续费实际扣除 vs 理论计算
  □ 1 周后 P&L vs 模拟盘 P&L 偏差 < 20%
```

### 3.2 扩展实盘（Week 6-7）

```
规模：10 城市 × 每笔 $5-10
资金：$300-500
新增：METAR 日内调仓上线

验证清单：
  □ 10 城市 并行运行稳定
  □ 日内调仓（确认加仓 + 危险止损）实际效果
  □ 批量下单 rate limit 无问题
  □ 2 周累计 P&L 与回测预期偏差 < 30%
```

### 3.3 全量（Week 8+）

```
规模：所有可用城市 × 最优参数
资金：$1,000-2,000
新增：Layer 3 YES 重定价

持续优化：
  - 每周重新跑参数扫描（用最近 2 周数据）
  - 城市排名更新
  - 异常检测（市场结构变化、费率变化）
```

---

## 城市配置（待 Phase 0 验证实际可用数量）

```python
# config/cities.py
#
# ⚠️ Phase 0 修正（2026-04-06）：
#   Polymarket 用 Wunderground 结算，气象站与直觉不同：
#     Dallas → KDAL（Love Field），不是 KDFW（DFW 机场）
#     NYC    → KLGA（LaGuardia），不是 KJFK（JFK 机场）
#   每新增城市前必须先查 Polymarket 市场描述里的 Wunderground URL 确认结算站。
#
# Gamma API slug 格式（已验证）：
#   highest-temperature-in-{polymarket_slug_city}-on-{month}-{dd}-2026
# 访问方式：GET https://gamma-api.polymarket.com/events?slug={slug}
#   注：?tag=weather 过滤器无效，勿用

CITIES = {
    # ── Phase 0 已验证（Chicago / Dallas / NYC）──
    "Chicago": {
        "lat": 41.88, "lon": -87.63,
        "icao": "KORD",                    # O'Hare — 与 Polymarket 结算站一致 ✅
        "tz": "America/Chicago",
        "polymarket_slug_city": "chicago",
        "wunderground_station": "KORD",
    },
    "Dallas": {
        "lat": 32.84, "lon": -96.85,       # Love Field 坐标（非 DFW）
        "icao": "KDAL",                    # Love Field ✅（原设计错误写了 KDFW）
        "tz": "America/Chicago",
        "polymarket_slug_city": "dallas",
        "wunderground_station": "KDAL",
    },
    "NYC": {
        "lat": 40.78, "lon": -73.87,       # LaGuardia 坐标（非 JFK）
        "icao": "KLGA",                    # LaGuardia ✅（原设计错误写了 KJFK）
        "tz": "America/New_York",
        "polymarket_slug_city": "nyc",
        "wunderground_station": "KLGA",
    },

    # ── Tier 1: 待验证（需先确认 Polymarket 结算站再启用）──
    # "London":   {"lat": 51.51, "lon": -0.13,  "icao": "EGLL", "tz": "Europe/London",      "polymarket_slug_city": "london"},
    # "Seoul":    {"lat": 37.57, "lon": 126.98, "icao": "RKSI", "tz": "Asia/Seoul",          "polymarket_slug_city": "seoul"},
    # "Toronto":  {"lat": 43.65, "lon": -79.38, "icao": "CYYZ", "tz": "America/Toronto",     "polymarket_slug_city": "toronto"},

    # ── Tier 2: 待验证 ──
    # "Ankara":   {"lat": 39.93, "lon": 32.86,  "icao": "LTAC", "tz": "Europe/Istanbul"},
    # "Miami":    {"lat": 25.76, "lon": -80.19, "icao": "KMIA", "tz": "America/New_York"},
    # "Atlanta":  {"lat": 33.75, "lon": -84.39, "icao": "KATL", "tz": "America/New_York"},
    # "Tokyo":    {"lat": 35.68, "lon": 139.69, "icao": "RJTT", "tz": "Asia/Tokyo"},
    # "Paris":    {"lat": 48.86, "lon": 2.35,   "icao": "LFPG", "tz": "Europe/Paris"},
    # "Istanbul": {"lat": 41.01, "lon": 28.98,  "icao": "LTFM", "tz": "Europe/Istanbul"},

    # ── Tier 3: 待确认（Phase 0 验证是否有市场）──
    # "Wellington", "Buenos_Aires", "Auckland" 等
}
```

---

## 依赖

```txt
# requirements.txt

# ── Phase 0: 数据层 ──
requests>=2.31.0               # HTTP 请求
schedule>=1.2.0                # 轻量调度（Phase 0 用，后期换 APScheduler）

# ── Phase 1: 分析层 ──
pandas>=2.0.0                  # 数据分析
numpy>=1.24.0                  # 数值计算
scipy>=1.11.0                  # 正态分布 + 统计检验
matplotlib>=3.7.0              # 可视化
jupyter>=1.0.0                 # Notebook

# ── Phase 1: ML 模型层 ──
scikit-learn>=1.3.0            # LR/Ridge + 交叉验证 + 评估指标
xgboost>=2.0.0                 # XGBoost 回归/分类
joblib>=1.3.0                  # 模型序列化

# ── Phase 2-3: 执行层 ──
py-clob-client==0.34.6         # Polymarket 官方 SDK
web3==6.14.0                   # py-clob-client 依赖
APScheduler>=3.10.0            # 生产级调度
python-telegram-bot>=20.0      # 通知

# ── 进阶（可选）──
cfgrib>=0.9.10                 # GEFS GRIB2 解析
xarray>=2023.0                 # 气象数据处理
```

---

## 风险清单

| 风险 | 影响 | 发现时机 | 应对 |
|------|------|---------|------|
| NO 流动性不够 | 策略无法执行 | Phase 0 ✅已验证 | 远端 bin $100 可成交，流动性充足 |
| 历史数据不够 | 回测不可靠 | Phase 0-1 | 延长录数据周期到 4 周 |
| 回测亏钱 | 策略无效 | Phase 1 | 重新审视 NO vs YES 比例 |
| 资本效率太低 | $500 不够 | Phase 1 | 提高本金 or 调整为 YES-heavy |
| 模拟盘 vs 回测偏差大 | 执行摩擦 | Phase 2 | 调整滑点假设和成交率 |
| Maker 挂单不成交 | 被迫用 Taker（1.25% fee） | Phase 2-3 | 调整挂单策略（更激进的价格） |
| 结算数据源不一致 | P&L 计算错误 | Phase 0 ✅已发现 | Wunderground 站确认：Dallas→KDAL，NYC→KLGA |
| 市场结构变化 | 参数失效 | 持续 | 每周重新跑参数扫描 |
| Polymarket 费率变化 | 利润压缩 | 持续 | 监控 changelog |
| Bot 崩溃 | 持仓无人管理 | Phase 3 | 崩溃恢复 + Telegram 告警 |

---

## 关键认知

```
1. 数据先行
   不要在 Phase 0 结束前写任何策略代码。
   先搞清楚数据长什么样，再决定策略怎么写。

2. 参数从回测来
   exclusion_range、price bounds、position size —— 全部用数据填。
   Phase 0 的设计文档里任何带具体数字的参数都是"待验证"。

3. NO 不一定是最优
   @chunxiangai 的 NO-first 是一种假说，不是定论。
   Phase 1 的资本效率分析可能发现 YES-heavy 更好。
   保持开放心态，让数据说话。

4. 慢就是快
   花 4 周做数据 + 回测 + 模拟，比直接上真钱亏 2 周更划算。
   Phase 0 回答的每一个问题，都可能避免 Phase 3 的一次爆雷。
```

---

---

## Phase 0 实侦记录（2026-04-06，Chicago / Dallas / NYC）

### 主要发现

```
1. Gamma API tag 过滤器无效
   ?tag=weather 被静默忽略。正确方式：按 slug 逐城市拉取。

2. 结算站与设计文档不同（最重要修正）
   Dallas: KDAL（Love Field），非 KDFW（DFW 机场）
   NYC:    KLGA（LaGuardia），非 KJFK（JFK 机场）
   → config/cities.py 已修正

3. 市场结构已全面核实
   - 11 bins / 城市，2°F 区间，华氏度整数精度
   - negRisk: true，clobTokenIds[1] 为 NO token
   - 直接 CLOB 购买 NO token 可行（方法 A）
   - 40+ 城市活跃市场（polymarket.com/weather）

4. NO token 三种路径全部验证
   - 方法 A（直接 CLOB 买 NO）：最优，秒级，fee = 5%×p×(1-p)
   - 方法 B（split+卖YES）：成本相同但慢，远端 bin YES 流动性不足
   - 方法 C（convertPositions）：再平衡工具，不适用于买入
   - 费率确认：Weather 类别 5% taker，maker 0%

5. 流动性充足
   远端 bin $100 可在最优价格一次成交，spread ≈ $0.005

5. 市场日历确认
   交易截止：目标日 12:00 UTC（本地早上），结算次日完成

6. 模型 vs 市场价格 — Chicago 异常（2026-04-07）
   ECMWF/GFS 预报约 33°F，但市场峰值 bin 为 38-39°F（38% YES）
   5°F 偏差，待 4 月 7 日 METAR 数据验证归因

7. Dallas 模型分歧高
   ECMWF 69°F vs GFS 74°F，差 2.5°C → 高不确定性，策略需谨慎
```

### 当前 Phase 0 状态

```
✅ API 侦察完成（Go/No-Go 前 5 项通过）
❌ 数据录制系统：待实现（market_recorder.py）
❌ 历史数据积累：待开始（目标：50+ 已结算市场）
```

---

*设计版本：v2.1 | Phase 0 实侦更新 | 2026年4月*
*基于：@chunxiangai NO-first 策略 + 排行榜数据 + Phase 0 API 实侦（Chicago/Dallas/NYC）*
*下一步：实现 data/market_recorder.py，开始录制数据*
