import { Routes, Route, NavLink, Navigate } from 'react-router-dom'
import LiveDashboard from './pages/LiveDashboard'
import BacktestDashboard from './pages/BacktestDashboard'

export default function App() {
  return (
    <div className="min-h-screen bg-gray-950 text-gray-100">
      <nav className="bg-gray-900 border-b border-gray-800 px-6 py-3 flex gap-6 items-center">
        <span className="font-semibold text-purple-400 mr-4">Polymarket Weather Bot</span>
        <NavLink
          to="/live"
          className={({ isActive }) =>
            `text-sm font-medium px-3 py-1 rounded transition ${isActive ? 'bg-purple-700 text-white' : 'text-gray-400 hover:text-white'}`
          }
        >
          Live Trading
        </NavLink>
        <NavLink
          to="/backtest"
          className={({ isActive }) =>
            `text-sm font-medium px-3 py-1 rounded transition ${isActive ? 'bg-purple-700 text-white' : 'text-gray-400 hover:text-white'}`
          }
        >
          Backtesting
        </NavLink>
      </nav>

      <main className="p-6">
        <Routes>
          <Route path="/" element={<Navigate to="/live" replace />} />
          <Route path="/live" element={<LiveDashboard />} />
          <Route path="/backtest" element={<BacktestDashboard />} />
        </Routes>
      </main>
    </div>
  )
}
