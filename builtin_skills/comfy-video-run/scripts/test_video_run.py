import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import video_run as v


GRAPH = {"24": {"class_type": "SaveVideo", "inputs": {"filename_prefix": "test"}}}
EXPECTED = {"width": 768, "height": 448, "frames": 121, "fps": 24}


def history(pid="job", graph=None):
    return {pid: {"prompt": [1, pid, graph or GRAPH],
                  "status": {"completed": True, "status_str": "success", "messages": []},
                  "outputs": {"24": {"images": [{"filename": "video.mp4", "subfolder": "safe", "type": "output"}]}}}}


class FakeClient:
    url = "http://localhost:8188"
    def __init__(self, data): self.data, self.downloads = data, 0
    def json(self, *args): return self.data
    def download(self, desc, path):
        self.downloads += 1
        path.write_bytes(b"new-video")


class RunTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / "run"
        v.initialize(self.root, "http://localhost:8188", {"graph": GRAPH, "outputs": {"24": EXPECTED}})
        v.save(self.root / "graph.json", GRAPH)
        self.record = v.event(self.root, "queued", prompt_id="job", graph_sha256=v.digest(v.canonical_graph(GRAPH)))

    def test_missing_history_never_delivers_old_master(self):
        old = self.root / "master.mp4"
        old.write_bytes(b"old")
        client = FakeClient({})
        result = v.collect(self.root, client)
        self.assertEqual(result["state"], "pending_or_unknown")
        self.assertEqual(result["artifacts"], [])
        self.assertEqual(client.downloads, 0)
        self.assertEqual(old.read_bytes(), b"old")

    def test_wrong_prompt_identity(self):
        h = history()
        h["job"]["prompt"][1] = "different"
        with self.assertRaisesRegex(ValueError, "identity mismatch"):
            v.history_entry(h, self.record)

    def test_changed_graph_rejected(self):
        g = copy.deepcopy(GRAPH)
        g["24"]["inputs"]["filename_prefix"] = "other"
        with self.assertRaisesRegex(ValueError, "graph differs"):
            v.history_entry(history(graph=g), self.record)

    def test_partial_outputs_not_success(self):
        h = history()
        h["job"]["status"]["completed"] = False
        self.assertIsNone(v.history_entry(h, self.record))

    def test_failure_with_outputs_rejected(self):
        h = history()
        h["job"]["status"]["messages"] = [["execution_error", {}]]
        with self.assertRaisesRegex(ValueError, "failed"):
            v.history_entry(h, self.record)

    def test_ambiguous_node_output_rejected(self):
        e = history()["job"]
        e["outputs"]["24"]["images"].append({"filename": "other.mp4", "subfolder": "", "type": "output"})
        with self.assertRaisesRegex(ValueError, "exactly one"):
            v.choose_output(e, "24")

    def test_selects_explicit_node_and_ignores_preview(self):
        e = history()["job"]
        e["outputs"]["25"] = {"images": [{"filename": "wrong.mp4", "subfolder": "", "type": "output"}]}
        e["outputs"]["24"]["images"].append({"filename": "preview.mp4", "subfolder": "", "type": "temp"})
        self.assertEqual(v.choose_output(e, "24")["filename"], "video.mp4")

    def test_path_traversal_rejected(self):
        for field, value in [("filename", "../video.mp4"), ("subfolder", "../escape"), ("filename", "C:\\video.mp4")]:
            e = history()["job"]
            e["outputs"]["24"]["images"][0][field] = value
            with self.assertRaisesRegex(ValueError, "Unsafe"):
                v.choose_output(e, "24")

    def test_frame_count_change_rejected(self):
        with self.assertRaisesRegex(ValueError, "frames"):
            v.check_probe({**EXPECTED, "frames": 137}, EXPECTED)

    @patch.object(v, "probe", return_value=EXPECTED)
    def test_verified_download_is_not_acceptance(self, probe):
        result = v.collect(self.root, FakeClient(history()), do_probes=False)
        self.assertEqual(result["state"], "review_pending")
        self.assertEqual(result["user_acceptance"], "pending")
        self.assertEqual(Path(result["artifacts"][0]["path"]).read_bytes(), b"new-video")
        self.assertFalse((self.root / "master.mp4").exists())

    @patch.object(v, "probe", return_value=EXPECTED)
    def test_gateway_output_manifest_on_collection_and_retry(self, probe):
        manifest = self.root / "gateway-artifacts.json"
        with patch.dict(v.os.environ, {"ARTIFACT_MANIFEST": str(manifest)}):
            result = v.collect(self.root, FakeClient(history()), do_probes=False)
            self.assertEqual(json.loads(manifest.read_text())["artifacts"], result["artifacts"])
            manifest.unlink()
            v.collect(self.root, FakeClient(history()), do_probes=False)
            self.assertEqual(json.loads(manifest.read_text())["artifacts"], result["artifacts"])

    @patch.object(v, "probe", return_value=EXPECTED)
    def test_idempotent_collection_checks_hash(self, probe):
        client = FakeClient(history())
        result = v.collect(self.root, client, do_probes=False)
        v.collect(self.root, client, do_probes=False)
        self.assertEqual(client.downloads, 1)
        Path(result["artifacts"][0]["path"]).write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "changed"):
            v.collect(self.root, client, do_probes=False)

    @patch.object(v, "probe", return_value={**EXPECTED, "frames": 137})
    def test_invalid_download_never_promoted(self, probe):
        with self.assertRaises(ValueError): v.collect(self.root, FakeClient(history()), do_probes=False)
        self.assertFalse((self.root / "outputs/node-24.mp4").exists())
        self.assertEqual(json.loads((self.root / "run.json").read_text())["state"], "verification_failed")

    def test_cannot_review_unverified_run(self):
        with self.assertRaisesRegex(ValueError, "Only verified"):
            v.review(self.root, {})

    def test_existing_run_directory_not_overwritten(self):
        with self.assertRaises(FileExistsError):
            v.initialize(self.root, "http://localhost:8188", {})

    def test_unknown_submission_not_retried(self):
        client = FakeClient({})
        client.json = lambda *args: (_ for _ in ()).throw(TimeoutError("lost response"))
        root = Path(self.tmp.name) / "new"
        with self.assertRaises(TimeoutError):
            v.submit(root, {"graph": GRAPH, "outputs": {"24": EXPECTED}}, client)
        self.assertEqual(json.loads((root / "run.json").read_text())["state"], "submission_unknown")
        with self.assertRaises(FileExistsError):
            v.submit(root, {"graph": GRAPH, "outputs": {"24": EXPECTED}}, client)


if __name__ == "__main__": unittest.main()
