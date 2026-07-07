# drission_sidecar

`drission_sidecar` is a tiny zero-dependency helper for Python scripts launched
by `drs identity-job run`. It does not automate the browser and does not own any
governance logic. Rust still owns profile leases, cooldowns, circuit breakers,
runtime risk, and audit ledgers.

Use it by putting this repo's `python/` directory on `PYTHONPATH`:

```bash
PYTHONPATH=/path/to/drission-rs/python \
drs --json identity-job run profile-assets.json \
  --per-asset \
  --child-result-dir child-results \
  --job-preset publish_conservative \
  -- python3 publish.py
```

Inside the existing Python business script:

```python
from drission_sidecar import asset, failed, profile_dir, succeeded

current = asset() or {}

try:
    # Keep the existing browser/business code here.
    publish_id = publish_with_profile(profile_dir())
except RateLimited:
    failed(
        "rate_limited",
        "platform returned rate limit",
        cooldown_seconds=900,
        next_state="repair",
    )
    raise SystemExit(1)

succeeded("published", result={"publishId": publish_id, "accountId": current.get("accountId")})
```

The helper writes the JSON file pointed to by `DRS_IDENTITY_RESULT_OUT`. The Rust
sidecar reads it and applies `job.failureReasonRules.<reason>` to release the
profile, cool it down, move it to `repair` / `quarantine`, and emit runtime risk
for the next run.
