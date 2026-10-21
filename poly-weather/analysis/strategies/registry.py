"""Strategy registry and config loading."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from .core import BaseStrategy
from .plugins import all_strategy_instances


DEFAULT_CONFIG = Path(__file__).with_name("strategies.json")
ALIASES = {
    "base_ev": "base_no_ev",
    "default": "base_no_ev",
}


def _load_config(path: Path = DEFAULT_CONFIG) -> dict[str, Any]:
    if not path.exists():
        return {"groups": {}, "strategies": {}}
    return json.loads(path.read_text(encoding="utf-8"))


def list_strategy_names(config_path: Path = DEFAULT_CONFIG) -> list[str]:
    configured = _load_config(config_path).get("strategies", {})
    names = set(all_strategy_instances())
    names.update(configured)
    return sorted(names)


def list_strategy_configs(config_path: Path = DEFAULT_CONFIG) -> list[dict[str, Any]]:
    config = _load_config(config_path)
    strategies = all_strategy_instances()
    rows = []
    for name in sorted(set(strategies) | set(config.get("strategies", {}))):
        strategy = strategies.get(name)
        cfg = dict(config.get("strategies", {}).get(name, {}))
        cfg.setdefault("name", name)
        cfg.setdefault("enabled", True)
        if strategy is not None:
            cfg.setdefault("required_data", list(strategy.required_data))
        rows.append(cfg)
    return rows


def resolve_strategy_name(strategy_name: str, params: dict[str, Any] | None = None) -> str:
    params = params or {}
    explicit = params.get("strategy_id") or params.get("plugin")
    if explicit:
        return str(explicit)
    if strategy_name.startswith("sweep_base_ev"):
        return "base_no_ev"
    return ALIASES.get(strategy_name, strategy_name)


def get_strategy(strategy_name: str, params: dict[str, Any] | None = None) -> BaseStrategy:
    resolved = resolve_strategy_name(strategy_name, params)
    strategies = all_strategy_instances()
    if resolved not in strategies:
        raise KeyError(f"Unknown strategy '{strategy_name}' resolved to '{resolved}'")
    return strategies[resolved]


def group_members(group_name: str, config_path: Path = DEFAULT_CONFIG) -> list[str]:
    config = _load_config(config_path)
    groups = config.get("groups", {})
    if group_name not in groups:
        raise KeyError(f"Unknown strategy group '{group_name}'")
    return [resolve_strategy_name(name) for name in groups[group_name]]


def load_strategy_params(strategy_name: str, config_path: Path = DEFAULT_CONFIG) -> dict[str, Any]:
    resolved = resolve_strategy_name(strategy_name)
    config = _load_config(config_path)
    defaults = dict(config.get("defaults", {}))
    specific = dict(config.get("strategies", {}).get(resolved, {}))
    params = {**defaults, **specific}
    params.pop("description", None)
    params.pop("required_data", None)
    params.setdefault("strategy_id", resolved)
    return params
