import { useState } from 'react'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  LineChart, Line, XAxis, YAxis, Tooltip, ResponsiveContainer, Legend,
} from 'recharts'

const API = '/api/backtest'
const CITY_OPTIONS = ['NYC', 'CHI', 'DAL'] as const

function parseCity(params: unknown) {
  if (typeof params !== 'string' || params.length === 0) return 'All'
  try {
    const parsed = JSON.parse(params) as { city?: unknown }
    return typeof parsed.city === 'string' && parsed.city.length > 0 ? parsed.city : 'All'
  } catch {
    return 'All'
  }
}

async function fetchJson(url: string) {
  const response = await fetch(url)
  const data = await response.json().catch(() => null)
  if (!response.ok) {
    const detail = typeof data?.detail === 'string' ? data.detail : `Request failed (${response.status})`
    throw new Error(detail)
  }
  return data
}

function useRuns() {
  return useQuery({
    queryKey: ['backtest-runs'],
    queryFn: () => fetchJson(`${API}/runs?limit=50`),
  })
}

function useStrategies() {
  return useQuery({
    queryKey: ['backtest-strategies'],
    queryFn: () => fetchJson(`${API}/strategies`),
  })
}

function useRunDetail(id: string | null) {
  return useQuery({
    queryKey: ['backtest-detail', id],
    queryFn: () => fetchJson(`${API}/${id}`),
    enabled: !!id,
  })
}

function fmt(n: number | null | undefined, d = 2) {
  if (n == null) return '—'
  return (n >= 0 ? '+' : '') + n.toFixed(d)
}

function MetricBadge({ label, value, highlight }: { label: string; value: string; highlight?: boolean }) {
  return (
    <div className={`rounded-lg p-3 border text-sm ${highlight ? 'bg-purple-900/30 border-purple-700' : 'bg-gray-800 border-gray-700'}`}>
      <div className="text-gray-400 text-xs mb-1">{label}</div>
      <div className="font-semibold text-white">{value}</div>
    </div>
  )
}

export default function BacktestDashboard() {
  const qc = useQueryClient()
  const runs = useRuns()
  const strategies = useStrategies()
  const runsList = Array.isArray(runs.data) ? runs.data : []
  const strategiesList = Array.isArray(strategies.data) ? strategies.data : []
  const strategyOptions = Array.from(
    new Map(
      strategiesList
        .filter((s: Record<string, unknown>) => typeof s.name === 'string' && (s.name as string).length > 0)
        .map((s: Record<string, unknown>) => [s.name as string, s]),
    ).values(),
  )

  const [strategy, setStrategy] = useState('base_ev')
  const [city, setCity] = useState<(typeof CITY_OPTIONS)[number]>('NYC')
  const [days, setDays] = useState(60)
  const [selectedId, setSelectedId] = useState<string | null>(null)

  const detail = useRunDetail(selectedId)

  const triggerRun = useMutation({
    mutationFn: () =>
      fetch(`${API}/run?strategy=${encodeURIComponent(strategy)}&days=${days}&city=${encodeURIComponent(city)}`, {
        method: 'POST',
      }).then(r => {
        if (!r.ok) return r.json().then(e => Promise.reject(e))
        return r.json()
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['backtest-runs'] }),
  })

  // Build PnL curves from selected run's trades
  const pnlCurve = (() => {
    if (!detail.data?.trades) return []
    const daily: Record<string, number> = {}
    for (const t of detail.data.trades) {
      daily[t.target_date] = (daily[t.target_date] ?? 0) + (t.pnl ?? 0)
    }
    let cum = 0
    return Object.entries(daily)
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([day, pnl]) => {
        cum += pnl
        return { day, pnl: +pnl.toFixed(4), cumulative: +cum.toFixed(4) }
      })
  })()

  const selectedRun = runsList.find((r: Record<string, unknown>) => r.id === selectedId)

  return (
    <div className="space-y-8">
      <h1 className="text-2xl font-bold text-white">Backtesting</h1>

      {/* Run form */}
      <section className="bg-gray-800 rounded-lg p-5 border border-gray-700">
        <h2 className="text-lg font-semibold text-gray-300 mb-4">Run a New Backtest</h2>
        <div className="flex flex-wrap gap-3 items-end">
          <div>
            <label className="block text-xs text-gray-400 mb-1">Strategy</label>
            <select
              value={strategy}
              onChange={e => setStrategy(e.target.value)}
              className="bg-gray-700 text-white rounded px-3 py-2 text-sm border border-gray-600 focus:outline-none"
            >
              {strategyOptions.map((s: Record<string, unknown>) => (
                <option key={s.name as string} value={s.name as string}>{s.name as string}</option>
              ))}
              {strategyOptions.length === 0 && (
                <option value="base_ev">base_ev</option>
              )}
            </select>
          </div>
          <div>
            <label className="block text-xs text-gray-400 mb-1">City</label>
            <select
              value={city}
              onChange={e => setCity(e.target.value as (typeof CITY_OPTIONS)[number])}
              className="bg-gray-700 text-white rounded px-3 py-2 text-sm border border-gray-600 focus:outline-none"
            >
              {CITY_OPTIONS.map(option => (
                <option key={option} value={option}>{option}</option>
              ))}
            </select>
          </div>
          <div>
            <label className="block text-xs text-gray-400 mb-1">Days</label>
            <input
              type="number"
              value={days}
              min={1} max={730}
              onChange={e => setDays(Number(e.target.value))}
              className="bg-gray-700 text-white rounded px-3 py-2 text-sm border border-gray-600 w-24 focus:outline-none"
            />
          </div>
          <button
            onClick={() => triggerRun.mutate()}
            disabled={triggerRun.isPending}
            className="px-4 py-2 bg-purple-700 hover:bg-purple-600 disabled:opacity-50 text-white rounded text-sm font-medium transition"
          >
            {triggerRun.isPending ? 'Running…' : 'Run Backtest'}
          </button>
          {triggerRun.isError && (
            <span className="text-red-400 text-sm">
              {(triggerRun.error as { detail?: string })?.detail ?? 'Error'}
            </span>
          )}
          {triggerRun.isSuccess && (
            <span className="text-green-400 text-sm">
              Done! {triggerRun.data?.city ?? city} · Sharpe: {triggerRun.data?.sharpe ?? '—'} · PnL: {fmt(triggerRun.data?.total_pnl)}
            </span>
          )}
        </div>
      </section>

      {/* Runs table */}
      <section>
        <h2 className="text-lg font-semibold text-gray-300 mb-3">All Runs</h2>
        {runs.isError && (
          <div className="mb-3 rounded-lg border border-red-800 bg-red-950/40 px-4 py-3 text-sm text-red-300">
            Failed to load backtest runs: {runs.error.message}
          </div>
        )}
        <div className="bg-gray-800 rounded-lg border border-gray-700 overflow-x-auto">
          <table className="w-full text-sm text-left">
            <thead className="text-gray-400 border-b border-gray-700">
              <tr>
                {['Strategy', 'City', 'Date Range', 'Trades', 'Win%', 'Total PnL', 'Sharpe', 'Status', ''].map(h => (
                  <th key={h} className="px-4 py-2 font-medium">{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {runsList.map((r: Record<string, unknown>) => (
                <tr
                  key={r.id as string}
                  className={`border-t border-gray-700 cursor-pointer ${selectedId === r.id ? 'bg-purple-900/20' : 'hover:bg-gray-750'}`}
                  onClick={() => setSelectedId(r.id as string)}
                >
                  <td className="px-4 py-2 font-medium text-purple-300">{r.strategy as string}</td>
                  <td className="px-4 py-2 text-xs text-gray-300">{parseCity(r.params)}</td>
                  <td className="px-4 py-2 text-xs text-gray-400">{r.date_from as string} → {r.date_to as string}</td>
                  <td className="px-4 py-2">{r.total_trades as number ?? '—'}</td>
                  <td className="px-4 py-2">{r.win_rate != null ? `${((r.win_rate as number) * 100).toFixed(1)}%` : '—'}</td>
                  <td className={`px-4 py-2 font-medium ${(r.total_pnl as number) >= 0 ? 'text-green-400' : 'text-red-400'}`}>
                    {fmt(r.total_pnl as number)}
                  </td>
                  <td className="px-4 py-2">{r.sharpe != null ? (r.sharpe as number).toFixed(2) : '—'}</td>
                  <td className="px-4 py-2">
                    <span className={`text-xs px-1.5 py-0.5 rounded ${r.status === 'done' ? 'bg-green-900 text-green-300' : r.status === 'running' ? 'bg-yellow-900 text-yellow-300' : 'bg-red-900 text-red-300'}`}>
                      {r.status as string}
                    </span>
                  </td>
                  <td className="px-4 py-2 text-purple-400 text-xs">view →</td>
                </tr>
              ))}
            </tbody>
          </table>
          {runsList.length === 0 && (
            <div className="px-4 py-6 text-gray-500 text-sm text-center">No backtest runs yet. Run one above.</div>
          )}
        </div>
      </section>

      {/* Detail panel */}
      {selectedId && (
        <section>
          <div className="flex items-center gap-4 mb-4">
            <h2 className="text-lg font-semibold text-gray-300">
              Run Detail: <span className="text-purple-300">{selectedRun?.strategy}</span>
            </h2>
            <button
              onClick={() => setSelectedId(null)}
              className="text-gray-500 hover:text-gray-300 text-sm"
            >
              ✕ close
            </button>
          </div>

          {detail.isLoading ? (
            <div className="text-gray-500 text-sm">Loading…</div>
          ) : (
            <div className="space-y-4">
              {/* Metrics */}
              <div className="grid grid-cols-2 gap-3 md:grid-cols-5">
                <MetricBadge label="City" value={parseCity(selectedRun?.params)} />
                <MetricBadge label="Trades" value={String(selectedRun?.total_trades ?? '—')} />
                <MetricBadge label="Win Rate" value={selectedRun?.win_rate != null ? `${(selectedRun.win_rate * 100).toFixed(1)}%` : '—'} />
                <MetricBadge label="Total PnL" value={fmt(selectedRun?.total_pnl) + ' USDC'} highlight={(selectedRun?.total_pnl ?? 0) >= 0} />
                <MetricBadge label="Sharpe" value={selectedRun?.sharpe != null ? selectedRun.sharpe.toFixed(2) : '—'} />
                <MetricBadge label="Date Range" value={`${selectedRun?.date_from} → ${selectedRun?.date_to}`} />
              </div>

              {/* PnL curve */}
              {pnlCurve.length > 0 && (
                <div className="bg-gray-800 rounded-lg p-4 border border-gray-700 h-56">
                  <div className="text-sm text-gray-400 mb-2">Cumulative PnL Curve</div>
                  <ResponsiveContainer width="100%" height="85%">
                    <LineChart data={pnlCurve}>
                      <XAxis dataKey="day" tick={{ fontSize: 10, fill: '#9ca3af' }} />
                      <YAxis tick={{ fontSize: 10, fill: '#9ca3af' }} />
                      <Tooltip
                        contentStyle={{ background: '#1f2937', border: '1px solid #374151', fontSize: 12 }}
                        formatter={(value, name) => [fmt(Number(value ?? 0)), String(name ?? '')]}
                      />
                      <Legend wrapperStyle={{ fontSize: 12 }} />
                      <Line dataKey="cumulative" stroke="#7c3aed" dot={false} strokeWidth={2} name="Cumulative PnL" />
                      <Line dataKey="pnl" stroke="#6b7280" dot={false} strokeWidth={1} name="Daily PnL" />
                    </LineChart>
                  </ResponsiveContainer>
                </div>
              )}

              {/* Trade log */}
              <div className="bg-gray-800 rounded-lg border border-gray-700 overflow-x-auto max-h-72 overflow-y-auto">
                <table className="w-full text-sm text-left">
                  <thead className="text-gray-400 border-b border-gray-700 sticky top-0 bg-gray-800">
                    <tr>
                      {['Date', 'Bin', 'Entry Price', 'Shares', 'Stake', 'PnL'].map(h => (
                        <th key={h} className="px-4 py-2 font-medium">{h}</th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {(detail.data?.trades ?? []).map((t: Record<string, unknown>, i: number) => (
                      <tr key={i} className="border-t border-gray-700">
                        <td className="px-4 py-1.5">{t.target_date as string}</td>
                        <td className="px-4 py-1.5">{t.bin_label as string}</td>
                        <td className="px-4 py-1.5">{(t.entry_price as number).toFixed(4)}</td>
                        <td className="px-4 py-1.5">{(t.shares as number).toFixed(1)}</td>
                        <td className="px-4 py-1.5">${(t.stake_usd as number).toFixed(2)}</td>
                        <td className={`px-4 py-1.5 font-medium ${(t.pnl as number) >= 0 ? 'text-green-400' : 'text-red-400'}`}>
                          {fmt(t.pnl as number)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          )}
        </section>
      )}
    </div>
  )
}
