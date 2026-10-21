import { useQuery } from '@tanstack/react-query'
import {
  BarChart, Bar, XAxis, YAxis, Tooltip, ResponsiveContainer, ReferenceLine,
} from 'recharts'

const API = '/api/live'
const FAST_REFRESH_MS = 15_000
const STANDARD_REFRESH_MS = 60_000

type Position = {
  id: number
  bin_label: string
  is_paper: boolean
  target_date: string
  entry_price: number
  shares: number
  stake_usd: number
  pnl: number | null
  strategy_name: string
}

type PnlPoint = {
  day: string
  daily_pnl: number
  cumulative_pnl: number
  trades: number
}

type Settlement = {
  city: string
  target_date: string
  winning_bin: string
  settlement_temp_f: number | null
  resolved_at: string | null
}

type StrategySummary = {
  name: string
  enabled?: boolean
  funding_pct?: number
  open_count?: number
  total_pnl?: number | null
}

type DailySummary = {
  city: string
  target_date: string
  nws_temp_f: number | null
  nws_short_forecast: string | null
  nws_period_name: string | null
  nws_timestamp: string | null
  open_meteo_ecmwf_f: number | null
  open_meteo_gfs_f: number | null
  open_meteo_spread_c: number | null
  open_meteo_timestamp: string | null
  market_snapshot_at: string | null
  market_bin_count: number
  market_bins_with_no_ask: number
  market_avg_no_best_ask: number | null
  market_min_no_best_ask: number | null
  market_max_no_best_ask: number | null
  event_slug: string | null
}

type MarketSnapshot = {
  id: number
  city: string
  target_date: string
  timestamp: string
  bin_label: string
  bin_index: number
  yes_price: number | null
  no_price: number | null
  no_best_bid: number | null
  no_best_ask: number | null
  book_depth_ask: number | null
  volume_24h: number | null
  liquidity: number | null
}

type MarketSnapshotGroup = {
  key: string
  city: string
  targetDate: string
  snapshotAt: string | null
  items: MarketSnapshot[]
}

async function fetchJson<T>(url: string): Promise<T> {
  const response = await fetch(url)
  if (!response.ok) {
    throw new Error(`Request failed: ${response.status}`)
  }
  return response.json() as Promise<T>
}

function usePositions(status: string, refetchInterval: number) {
  return useQuery<Position[]>({
    queryKey: ['positions', status],
    queryFn: () => fetchJson<Position[]>(`${API}/positions?status=${status}&limit=200`),
    refetchInterval,
  })
}

function usePnl() {
  return useQuery<PnlPoint[]>({
    queryKey: ['pnl'],
    queryFn: () => fetchJson<PnlPoint[]>(`${API}/pnl?days=30`),
    refetchInterval: STANDARD_REFRESH_MS,
  })
}

function useSettlements() {
  return useQuery<Settlement[]>({
    queryKey: ['settlements'],
    queryFn: () => fetchJson<Settlement[]>(`${API}/settlements?days=30`),
    refetchInterval: STANDARD_REFRESH_MS,
  })
}

function useStrategies() {
  return useQuery<StrategySummary[]>({
    queryKey: ['live-strategies'],
    queryFn: () => fetchJson<StrategySummary[]>(`${API}/strategies`),
    refetchInterval: FAST_REFRESH_MS,
  })
}

function useDailySummary() {
  return useQuery<DailySummary[]>({
    queryKey: ['daily-summary'],
    queryFn: () => fetchJson<DailySummary[]>(`${API}/daily-summary?days=3`),
    refetchInterval: STANDARD_REFRESH_MS,
  })
}

function useSnapshots() {
  return useQuery<MarketSnapshot[]>({
    queryKey: ['snapshots-latest'],
    queryFn: () => fetchJson<MarketSnapshot[]>(`${API}/snapshots/latest?days=2`),
    refetchInterval: FAST_REFRESH_MS,
  })
}

function fmtSigned(n: number | null | undefined, decimals = 2) {
  if (n == null) return '—'
  return `${n >= 0 ? '+' : ''}${n.toFixed(decimals)}`
}

function fmtNumber(n: number | null | undefined, decimals = 2) {
  if (n == null) return '—'
  return n.toFixed(decimals)
}

function fmtTemp(n: number | null | undefined) {
  if (n == null) return '—'
  return `${n.toFixed(1)}°F`
}

function fmtTimestamp(value: string | null | undefined) {
  if (!value) return '—'
  return new Date(value).toLocaleString([], {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  })
}

function fmtDate(value: string) {
  return new Date(`${value}T00:00:00`).toLocaleDateString([], {
    month: 'short',
    day: 'numeric',
  })
}

function groupSnapshots(items: MarketSnapshot[]): MarketSnapshotGroup[] {
  const groups = new Map<string, MarketSnapshotGroup>()

  for (const item of items) {
    const key = `${item.city}:${item.target_date}`
    const existing = groups.get(key)
    if (existing) {
      existing.items.push(item)
      if (!existing.snapshotAt || item.timestamp > existing.snapshotAt) {
        existing.snapshotAt = item.timestamp
      }
      continue
    }

    groups.set(key, {
      key,
      city: item.city,
      targetDate: item.target_date,
      snapshotAt: item.timestamp,
      items: [item],
    })
  }

  return Array.from(groups.values()).sort((a, b) => {
    const cityCompare = a.city.localeCompare(b.city)
    if (cityCompare !== 0) return cityCompare
    return a.targetDate.localeCompare(b.targetDate)
  })
}

function PositionCard({ pos }: { pos: Position }) {
  return (
    <div className="bg-gray-800 rounded-lg p-4 border border-gray-700 text-sm">
      <div className="flex justify-between items-start mb-2">
        <span className="font-medium text-white">{pos.bin_label}</span>
        <span className={`text-xs px-2 py-0.5 rounded ${pos.is_paper ? 'bg-blue-900 text-blue-300' : 'bg-green-900 text-green-300'}`}>
          {pos.is_paper ? 'paper' : 'live'}
        </span>
      </div>
      <div className="text-gray-400 space-y-1">
        <div>Date: {pos.target_date}</div>
        <div>Entry: {pos.entry_price.toFixed(4)} · Shares: {pos.shares.toFixed(1)}</div>
        <div>Stake: ${pos.stake_usd.toFixed(2)}</div>
        {pos.pnl != null && (
          <div className={pos.pnl >= 0 ? 'text-green-400' : 'text-red-400'}>
            PnL: {fmtSigned(pos.pnl)}
          </div>
        )}
        <div className="text-purple-400 text-xs">{pos.strategy_name}</div>
      </div>
    </div>
  )
}

function DailySummaryCard({ item }: { item: DailySummary }) {
  const marketLive = item.market_bin_count > 0

  return (
    <div className="bg-gray-800 rounded-xl border border-gray-700 p-4 space-y-4">
      <div className="flex items-start justify-between gap-3">
        <div>
          <div className="text-xs uppercase tracking-wide text-gray-500">{item.city}</div>
          <div className="text-lg font-semibold text-white">{fmtDate(item.target_date)}</div>
        </div>
        <span className={`text-xs px-2 py-1 rounded-full ${marketLive ? 'bg-emerald-900 text-emerald-300' : 'bg-gray-700 text-gray-300'}`}>
          {marketLive ? `${item.market_bin_count} live bins` : 'No live market'}
        </span>
      </div>

      <div className="grid grid-cols-2 gap-4">
        <div>
          <div className="text-xs uppercase tracking-wide text-gray-500 mb-1">NWS</div>
          <div className="text-2xl font-semibold text-white">{fmtTemp(item.nws_temp_f)}</div>
          <div className="text-sm text-gray-400">{item.nws_short_forecast ?? '—'}</div>
          <div className="text-xs text-gray-500 mt-1">
            {item.nws_period_name ?? '—'} · {fmtTimestamp(item.nws_timestamp)}
          </div>
        </div>
        <div>
          <div className="text-xs uppercase tracking-wide text-gray-500 mb-1">Open-Meteo</div>
          <div className="text-sm text-gray-300">ECMWF: {fmtTemp(item.open_meteo_ecmwf_f)}</div>
          <div className="text-sm text-gray-300">GFS: {fmtTemp(item.open_meteo_gfs_f)}</div>
          <div className="text-xs text-gray-500 mt-1">
            Spread: {item.open_meteo_spread_c != null ? `${item.open_meteo_spread_c.toFixed(2)}°C` : '—'}
          </div>
          <div className="text-xs text-gray-500">{fmtTimestamp(item.open_meteo_timestamp)}</div>
        </div>
      </div>

      <div className="rounded-lg bg-gray-900/70 border border-gray-700 p-3 text-xs text-gray-300 space-y-1">
        <div>Latest market snapshot: {fmtTimestamp(item.market_snapshot_at)}</div>
        <div>No ask coverage: {item.market_bins_with_no_ask}/{item.market_bin_count}</div>
        <div>
          NO ask range: {item.market_min_no_best_ask != null ? item.market_min_no_best_ask.toFixed(3) : '—'}
          {' '}to{' '}
          {item.market_max_no_best_ask != null ? item.market_max_no_best_ask.toFixed(3) : '—'}
        </div>
        <div>Average NO ask: {fmtNumber(item.market_avg_no_best_ask, 3)}</div>
      </div>
    </div>
  )
}

export default function LiveDashboard() {
  const open = usePositions('open', FAST_REFRESH_MS)
  const closed = usePositions('closed', STANDARD_REFRESH_MS)
  const pnl = usePnl()
  const settlements = useSettlements()
  const strategies = useStrategies()
  const dailySummary = useDailySummary()
  const snapshots = useSnapshots()

  const totalPnl = (pnl.data ?? []).reduce((sum, point) => sum + (point.daily_pnl ?? 0), 0)
  const snapshotGroups = groupSnapshots(snapshots.data ?? [])

  return (
    <div className="space-y-8">
      <div className="flex flex-col gap-2 md:flex-row md:items-end md:justify-between">
        <div>
          <h1 className="text-2xl font-bold text-white">Live Trading</h1>
          <p className="text-sm text-gray-400">
            Forecast cards refresh every minute. Positions, strategy stats, and live books refresh every 15 seconds.
          </p>
        </div>
      </div>

      <section>
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-lg font-semibold text-gray-300">Next 3 Days</h2>
          <span className="text-xs text-gray-500">NWS + Open-Meteo + market availability</span>
        </div>
        {dailySummary.isLoading ? (
          <div className="text-gray-500 text-sm">Loading…</div>
        ) : (
          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            {(dailySummary.data ?? []).map((item) => (
              <DailySummaryCard key={`${item.city}-${item.target_date}`} item={item} />
            ))}
          </div>
        )}
      </section>

      {strategies.data && strategies.data.length > 0 && (
        <section>
          <h2 className="text-lg font-semibold text-gray-300 mb-3">Strategies</h2>
          <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
            {strategies.data.map((strategy) => (
              <div key={strategy.name} className="bg-gray-800 rounded-lg p-3 border border-gray-700 text-sm">
                <div className="font-medium text-white mb-1">{strategy.name}</div>
                <div className="text-gray-400 text-xs space-y-0.5">
                  <div>Enabled: {strategy.enabled ? 'yes' : 'no'}</div>
                  <div>Funding: {strategy.funding_pct != null ? `${(strategy.funding_pct * 100).toFixed(0)}%` : '—'}</div>
                  <div>Open: {Number(strategy.open_count ?? 0)}</div>
                  <div className={(strategy.total_pnl ?? 0) >= 0 ? 'text-green-400' : 'text-red-400'}>
                    PnL: {fmtSigned(strategy.total_pnl)}
                  </div>
                </div>
              </div>
            ))}
          </div>
        </section>
      )}

      <section>
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-lg font-semibold text-gray-300">Live Market Board</h2>
          <span className="text-xs text-gray-500">Latest bin snapshot per city and target date</span>
        </div>
        {snapshots.isLoading ? (
          <div className="text-gray-500 text-sm">Loading…</div>
        ) : snapshotGroups.length === 0 ? (
          <div className="text-gray-500 text-sm">No market snapshots yet.</div>
        ) : (
          <div className="grid gap-4 xl:grid-cols-2">
            {snapshotGroups.map((group) => (
              <div key={group.key} className="bg-gray-800 rounded-lg border border-gray-700 overflow-x-auto">
                <div className="px-4 py-3 border-b border-gray-700 flex items-center justify-between gap-3">
                  <div>
                    <div className="text-white font-medium">{group.city} · {fmtDate(group.targetDate)}</div>
                    <div className="text-xs text-gray-500">Snapshot: {fmtTimestamp(group.snapshotAt)}</div>
                  </div>
                  <div className="text-xs text-gray-400">{group.items.length} bins</div>
                </div>
                <table className="w-full text-sm text-left">
                  <thead className="text-gray-400 border-b border-gray-700">
                    <tr>
                      {['Bin', 'YES', 'NO', 'NO Bid', 'NO Ask', 'Ask Depth', 'Vol 24h', 'Liquidity'].map((header) => (
                        <th key={header} className="px-4 py-2 font-medium">{header}</th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {group.items.map((item) => (
                      <tr key={item.id} className="border-t border-gray-700">
                        <td className="px-4 py-2 font-medium text-white">{item.bin_label}</td>
                        <td className="px-4 py-2">{fmtNumber(item.yes_price, 3)}</td>
                        <td className="px-4 py-2">{fmtNumber(item.no_price, 3)}</td>
                        <td className="px-4 py-2">{fmtNumber(item.no_best_bid, 3)}</td>
                        <td className="px-4 py-2 text-emerald-300">{fmtNumber(item.no_best_ask, 3)}</td>
                        <td className="px-4 py-2">{fmtNumber(item.book_depth_ask, 1)}</td>
                        <td className="px-4 py-2">{fmtNumber(item.volume_24h, 1)}</td>
                        <td className="px-4 py-2">{fmtNumber(item.liquidity, 1)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            ))}
          </div>
        )}
      </section>

      <section>
        <div className="flex items-center gap-4 mb-3">
          <h2 className="text-lg font-semibold text-gray-300">30-Day PnL</h2>
          <span className={`text-sm font-medium ${totalPnl >= 0 ? 'text-green-400' : 'text-red-400'}`}>
            Cumulative: {fmtSigned(totalPnl)} USDC
          </span>
        </div>
        <div className="bg-gray-800 rounded-lg p-4 border border-gray-700 h-48">
          {pnl.isLoading ? (
            <div className="text-gray-500 text-sm">Loading…</div>
          ) : (
            <ResponsiveContainer width="100%" height="100%">
              <BarChart data={pnl.data ?? []}>
                <XAxis dataKey="day" tick={{ fontSize: 10, fill: '#9ca3af' }} />
                <YAxis tick={{ fontSize: 10, fill: '#9ca3af' }} />
                <Tooltip
                  contentStyle={{ background: '#1f2937', border: '1px solid #374151', fontSize: 12 }}
                  formatter={(value) => [fmtSigned(Number(value ?? 0)), 'PnL']}
                />
                <ReferenceLine y={0} stroke="#4b5563" />
                <Bar dataKey="daily_pnl" fill="#7c3aed" radius={[2, 2, 0, 0]} />
              </BarChart>
            </ResponsiveContainer>
          )}
        </div>
      </section>

      <section>
        <h2 className="text-lg font-semibold text-gray-300 mb-3">
          Open Positions ({open.data?.length ?? 0})
        </h2>
        {open.isLoading ? (
          <div className="text-gray-500 text-sm">Loading…</div>
        ) : open.data?.length === 0 ? (
          <div className="text-gray-500 text-sm">No open positions.</div>
        ) : (
          <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
            {(open.data ?? []).map((position) => (
              <PositionCard key={position.id} pos={position} />
            ))}
          </div>
        )}
      </section>

      <section>
        <h2 className="text-lg font-semibold text-gray-300 mb-3">Settlement History (30d)</h2>
        <div className="bg-gray-800 rounded-lg border border-gray-700 overflow-x-auto">
          <table className="w-full text-sm text-left">
            <thead className="text-gray-400 border-b border-gray-700">
              <tr>
                {['Date', 'City', 'Winning Bin', 'Temp °F', 'Resolved At'].map((header) => (
                  <th key={header} className="px-4 py-2 font-medium">{header}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {(settlements.data ?? []).map((settlement) => (
                <tr key={`${settlement.city}-${settlement.target_date}-${settlement.winning_bin}`} className="border-t border-gray-700 hover:bg-gray-750">
                  <td className="px-4 py-2">{settlement.target_date}</td>
                  <td className="px-4 py-2 capitalize">{settlement.city}</td>
                  <td className="px-4 py-2 font-medium text-purple-300">{settlement.winning_bin}</td>
                  <td className="px-4 py-2">{fmtTemp(settlement.settlement_temp_f)}</td>
                  <td className="px-4 py-2 text-gray-500 text-xs">{settlement.resolved_at ?? '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {(settlements.data ?? []).length === 0 && (
            <div className="px-4 py-6 text-gray-500 text-sm text-center">No settlements in the last 30 days.</div>
          )}
        </div>
      </section>

      <section>
        <h2 className="text-lg font-semibold text-gray-300 mb-3">
          Recent Closed ({closed.data?.length ?? 0})
        </h2>
        <div className="bg-gray-800 rounded-lg border border-gray-700 overflow-x-auto">
          <table className="w-full text-sm text-left">
            <thead className="text-gray-400 border-b border-gray-700">
              <tr>
                {['Date', 'Bin', 'Strategy', 'Stake', 'PnL', 'Type'].map((header) => (
                  <th key={header} className="px-4 py-2 font-medium">{header}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {(closed.data ?? []).slice(0, 20).map((position) => (
                <tr key={position.id} className="border-t border-gray-700">
                  <td className="px-4 py-2">{position.target_date}</td>
                  <td className="px-4 py-2">{position.bin_label}</td>
                  <td className="px-4 py-2 text-purple-400 text-xs">{position.strategy_name}</td>
                  <td className="px-4 py-2">${position.stake_usd.toFixed(2)}</td>
                  <td className={`px-4 py-2 font-medium ${(position.pnl ?? 0) >= 0 ? 'text-green-400' : 'text-red-400'}`}>
                    {fmtSigned(position.pnl)}
                  </td>
                  <td className="px-4 py-2">
                    <span className={`text-xs px-1.5 py-0.5 rounded ${position.is_paper ? 'bg-blue-900 text-blue-300' : 'bg-green-900 text-green-300'}`}>
                      {position.is_paper ? 'paper' : 'live'}
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {(closed.data ?? []).length === 0 && (
            <div className="px-4 py-6 text-gray-500 text-sm text-center">No closed positions yet.</div>
          )}
        </div>
      </section>
    </div>
  )
}
