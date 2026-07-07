"""Tiny Python helper for ``drs identity-job run`` child scripts.

The Rust sidecar owns leases, cooldowns, circuit breakers, and audit ledgers.
This module only reads the environment injected by ``drs`` and writes the
child result JSON protocol expected by ``identity-job run``.
"""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path
from typing import Any, Dict, List, Mapping, Optional

__all__ = [
    "SidecarError",
    "account_id",
    "asset",
    "cancelled",
    "context",
    "env",
    "failed",
    "identity_id",
    "label",
    "profile_dir",
    "profile_id",
    "require_asset",
    "result_path",
    "runtime_lease_id",
    "selected_assets",
    "selected_count",
    "succeeded",
    "write_result",
]

__version__ = "0.1.0"

_VALID_STATUSES = {"succeeded", "failed", "cancelled"}


class SidecarError(RuntimeError):
    """Raised when the child process is not running under ``identity-job``."""


def env(name: str, default: Optional[str] = None) -> Optional[str]:
    """Return an injected ``DRS_*`` environment variable."""

    return os.environ.get(name, default)


def _parse_json_env(name: str, default: Any) -> Any:
    raw = os.environ.get(name)
    if raw is None or raw == "":
        return default
    try:
        return json.loads(raw)
    except json.JSONDecodeError as exc:
        raise SidecarError(f"{name} is not valid JSON: {exc}") from exc


def asset(default: Optional[Dict[str, Any]] = None) -> Optional[Dict[str, Any]]:
    """Return the per-asset payload injected as ``DRS_IDENTITY_ASSET_JSON``."""

    value = _parse_json_env("DRS_IDENTITY_ASSET_JSON", default)
    if value is None:
        return None
    if not isinstance(value, dict):
        raise SidecarError("DRS_IDENTITY_ASSET_JSON must be a JSON object")
    return value


def require_asset() -> Dict[str, Any]:
    """Return the current asset or raise when not running in per-asset mode."""

    value = asset()
    if value is None:
        raise SidecarError("DRS_IDENTITY_ASSET_JSON is missing")
    return value


def selected_assets() -> List[Dict[str, Any]]:
    """Return all selected assets from ``DRS_IDENTITY_SELECTED_ASSETS_JSON``."""

    value = _parse_json_env("DRS_IDENTITY_SELECTED_ASSETS_JSON", [])
    if not isinstance(value, list):
        raise SidecarError("DRS_IDENTITY_SELECTED_ASSETS_JSON must be a JSON array")
    for item in value:
        if not isinstance(item, dict):
            raise SidecarError("DRS_IDENTITY_SELECTED_ASSETS_JSON items must be objects")
    return value


def selected_count() -> int:
    """Return ``DRS_IDENTITY_SELECTED_COUNT`` as an integer."""

    raw = os.environ.get("DRS_IDENTITY_SELECTED_COUNT", "0")
    try:
        return int(raw)
    except ValueError as exc:
        raise SidecarError("DRS_IDENTITY_SELECTED_COUNT must be an integer") from exc


def _asset_value(field: str, env_name: str) -> Optional[str]:
    direct = os.environ.get(env_name)
    if direct:
        return direct
    current = asset()
    value = current.get(field) if current else None
    return str(value) if value is not None else None


def account_id() -> Optional[str]:
    return _asset_value("accountId", "DRS_IDENTITY_ACCOUNT_ID")


def profile_id() -> Optional[str]:
    return _asset_value("profileId", "DRS_IDENTITY_PROFILE_ID")


def identity_id() -> Optional[str]:
    return _asset_value("identityId", "DRS_IDENTITY_IDENTITY_ID")


def label() -> Optional[str]:
    return _asset_value("label", "DRS_IDENTITY_LABEL")


def profile_dir() -> Optional[str]:
    return _asset_value("profileDir", "DRS_IDENTITY_PROFILE_DIR")


def runtime_lease_id() -> Optional[str]:
    return _asset_value("leaseId", "DRS_IDENTITY_RUNTIME_LEASE_ID")


def result_path(required: bool = True) -> Optional[Path]:
    """Return the child result path injected by ``identity-job run``."""

    raw = os.environ.get("DRS_IDENTITY_RESULT_OUT") or os.environ.get(
        "DRS_IDENTITY_CHILD_RESULT_OUT"
    )
    if raw:
        return Path(raw)
    if required:
        raise SidecarError(
            "DRS_IDENTITY_RESULT_OUT is missing; run the script via "
            "`drs identity-job run --child-result-dir ... -- python script.py`"
        )
    return None


def context() -> Dict[str, Any]:
    """Return the useful ``identity-job`` context as one dictionary."""

    return {
        "jobRunId": os.environ.get("DRS_IDENTITY_JOB_RUN_ID"),
        "worker": os.environ.get("DRS_IDENTITY_WORKER"),
        "job": os.environ.get("DRS_IDENTITY_JOB"),
        "assetManifest": os.environ.get("DRS_IDENTITY_ASSET_MANIFEST"),
        "selectionOut": os.environ.get("DRS_IDENTITY_SELECTION_OUT"),
        "childIndex": os.environ.get("DRS_IDENTITY_CHILD_INDEX"),
        "selectedCount": selected_count(),
        "selectedAssets": selected_assets(),
        "asset": asset(),
        "resultOut": str(result_path(required=False) or ""),
    }


def _atomic_write_json(path: Path, payload: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    data = json.dumps(payload, ensure_ascii=False, separators=(",", ":")) + "\n"
    handle, tmp_name = tempfile.mkstemp(
        prefix=f".{path.name}.",
        suffix=".tmp",
        dir=str(path.parent),
        text=True,
    )
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as file:
            file.write(data)
        os.replace(tmp_name, path)
    except Exception:
        try:
            os.unlink(tmp_name)
        except FileNotFoundError:
            pass
        raise


def write_result(
    status: str,
    *,
    reason: Optional[str] = None,
    message: Optional[str] = None,
    result: Any = None,
    cooldown_seconds: Optional[int] = None,
    next_state: Optional[str] = None,
    path: Optional[Path] = None,
    **extra: Any,
) -> Dict[str, Any]:
    """Write the child result JSON consumed by ``identity-job run``.

    ``status`` must be ``succeeded``, ``failed``, or ``cancelled``. ``reason`` is
    the key that ``job.failureReasonRules.<reason>`` uses for cooldowns,
    next-state changes, and runtime-risk decisions.
    """

    normalized_status = status.strip().lower().replace("-", "_")
    if normalized_status not in _VALID_STATUSES:
        raise ValueError(
            "status must be one of succeeded, failed, or cancelled "
            f"(got {status!r})"
        )
    if cooldown_seconds is not None and cooldown_seconds <= 0:
        raise ValueError("cooldown_seconds must be greater than 0")

    payload: Dict[str, Any] = {"status": normalized_status}
    if message is not None:
        payload["message"] = message
    if reason is not None:
        payload["reason"] = reason
    if result is not None:
        payload["result"] = result
    if cooldown_seconds is not None:
        payload["cooldownSeconds"] = cooldown_seconds
    if next_state is not None:
        payload["nextState"] = next_state
    payload.update(extra)

    target = path or result_path(required=True)
    assert target is not None
    _atomic_write_json(Path(target), payload)
    return payload


def succeeded(
    message: Optional[str] = None,
    *,
    result: Any = None,
    path: Optional[Path] = None,
    **extra: Any,
) -> Dict[str, Any]:
    """Write a successful child result."""

    return write_result(
        "succeeded",
        message=message,
        result=result,
        path=path,
        **extra,
    )


def failed(
    reason: str,
    message: Optional[str] = None,
    *,
    result: Any = None,
    cooldown_seconds: Optional[int] = None,
    next_state: Optional[str] = None,
    path: Optional[Path] = None,
    **extra: Any,
) -> Dict[str, Any]:
    """Write a failed child result with a governance reason."""

    if not reason:
        raise ValueError("reason is required for failed results")
    return write_result(
        "failed",
        reason=reason,
        message=message,
        result=result,
        cooldown_seconds=cooldown_seconds,
        next_state=next_state,
        path=path,
        **extra,
    )


def cancelled(
    reason: Optional[str] = None,
    message: Optional[str] = None,
    *,
    result: Any = None,
    path: Optional[Path] = None,
    **extra: Any,
) -> Dict[str, Any]:
    """Write a cancelled child result."""

    return write_result(
        "cancelled",
        reason=reason,
        message=message,
        result=result,
        path=path,
        **extra,
    )
