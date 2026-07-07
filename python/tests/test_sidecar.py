import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import drission_sidecar as sidecar


class SidecarTests(unittest.TestCase):
    def test_asset_context_and_accessors(self):
        asset = {
            "accountId": "acct-a",
            "profileId": "profile-a",
            "identityId": "fp-a",
            "label": "acct-a",
            "profileDir": "/profiles/acct-a",
            "leaseId": "lease-a",
        }
        env = {
            "DRS_IDENTITY_JOB_RUN_ID": "run-a",
            "DRS_IDENTITY_WORKER": "worker-a",
            "DRS_IDENTITY_JOB": "publish",
            "DRS_IDENTITY_SELECTED_COUNT": "1",
            "DRS_IDENTITY_SELECTED_ASSETS_JSON": json.dumps([asset]),
            "DRS_IDENTITY_ASSET_JSON": json.dumps(asset),
        }
        with patch.dict(os.environ, env, clear=True):
            self.assertEqual(sidecar.account_id(), "acct-a")
            self.assertEqual(sidecar.profile_id(), "profile-a")
            self.assertEqual(sidecar.identity_id(), "fp-a")
            self.assertEqual(sidecar.label(), "acct-a")
            self.assertEqual(sidecar.profile_dir(), "/profiles/acct-a")
            self.assertEqual(sidecar.runtime_lease_id(), "lease-a")
            self.assertEqual(sidecar.selected_count(), 1)
            self.assertEqual(sidecar.selected_assets(), [asset])
            self.assertEqual(sidecar.context()["job"], "publish")

    def test_succeeded_writes_result_out(self):
        with tempfile.TemporaryDirectory() as temp:
            result_out = Path(temp) / "child-result.json"
            env = {"DRS_IDENTITY_RESULT_OUT": str(result_out)}
            with patch.dict(os.environ, env, clear=True):
                payload = sidecar.succeeded("done", result={"itemId": "item-a"})

            self.assertEqual(
                payload,
                {
                    "status": "succeeded",
                    "message": "done",
                    "result": {"itemId": "item-a"},
                },
            )
            self.assertEqual(json.loads(result_out.read_text()), payload)

    def test_failed_writes_governance_reason(self):
        with tempfile.TemporaryDirectory() as temp:
            result_out = Path(temp) / "nested" / "child-result.json"
            payload = sidecar.failed(
                "rate_limited",
                "limited",
                cooldown_seconds=900,
                next_state="repair",
                path=result_out,
            )

            self.assertEqual(payload["status"], "failed")
            self.assertEqual(payload["reason"], "rate_limited")
            self.assertEqual(payload["cooldownSeconds"], 900)
            self.assertEqual(payload["nextState"], "repair")
            self.assertEqual(json.loads(result_out.read_text()), payload)

    def test_missing_result_out_raises(self):
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(sidecar.SidecarError):
                sidecar.succeeded("done")

    def test_invalid_asset_json_raises(self):
        with patch.dict(os.environ, {"DRS_IDENTITY_ASSET_JSON": "not-json"}, clear=True):
            with self.assertRaises(sidecar.SidecarError):
                sidecar.asset()


if __name__ == "__main__":
    unittest.main()
