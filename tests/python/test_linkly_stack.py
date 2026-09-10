"""Compose and .env patching for the Linkly stack helper."""
import importlib.util
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / "scripts/linkly_stack.py"
spec = importlib.util.spec_from_file_location("linkly_stack", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

TEMPLATE_COMPOSE = """namespace: default
startup_timeout: 360s

containers:
  http:
    worker: package://http
    version: "0.21.9"
    config_name: http

  console:
    worker: package://console
    version: "1.9.24"
  shell:
    worker: package://shell
    version: "0.12.5"
    working_dir: .
  session-manager:
    worker: package://session-manager
    version: "1.0.17"
  iii-directory:
    worker: package://iii-directory
    version: "1.2.11"
  llm-router:
    worker: package://llm-router
    version: "1.4.19"
    start_after: [state]
    env_file: ['./.env']
  provider-anthropic:
    worker: package://provider-anthropic
    version: "1.2.16"
    env_file: ['./.env']
    start_after: [state, llm-router]

  #  provider-deepseek:            # DEEPSEEK_API_KEY
  #    worker: package://provider-deepseek
  #    version: "0.1.11"
  #    start_after: [state, llm-router]

  #  provider-kimi:                # MOONSHOT_API_KEY, not KIMI_API_KEY
  #    worker: package://provider-kimi
  #    version: "1.1.10"
  #    start_after: [state, llm-router]

  harness:
    worker: package://harness
    version: "1.8.17"
    start_after:
      - context-manager
      - provider-anthropic
      - provider-openai
      - state

  # Ch. 3: SQLite storage for links and clicks.
  # database:
  #   worker: package://database
"""

TEMPLATE_ENV = """# OpenAI and Anthropic providers are included by default.
ANTHROPIC_API_KEY=your-anthropic-key
OPENAI_API_KEY=your-openai-key

# Uncomment the line for each extra provider you enable in worker-compose.yaml.
# DEEPSEEK_API_KEY=your-deepseek-key
# MOONSHOT_API_KEY=your-moonshot-key
"""


class ComposePatchTests(unittest.TestCase):
    def test_commented_provider_is_enabled_with_env_file_and_start_after(self):
        patched = module.patch_compose(TEMPLATE_COMPOSE, ["deepseek"])
        self.assertIn("  provider-deepseek:            # DEEPSEEK_API_KEY\n"
                      "    worker: package://provider-deepseek\n"
                      '    version: "0.1.11"\n'
                      "    start_after: [state, llm-router]\n"
                      "    env_file: ['./.env']\n", patched)
        self.assertIn("  #  provider-kimi:", patched)
        start_after = patched.split("    start_after:\n", 1)[1].split("\n\n", 1)[0]
        self.assertEqual(
            start_after.splitlines(),
            ["      - context-manager", "      - provider-anthropic", "      - provider-deepseek",
             "      - provider-openai", "      - state"],
        )
        # the chapter comments below the harness block are untouched
        self.assertIn("  # Ch. 3: SQLite storage for links and clicks.\n  # database:", patched)

    def test_shipped_provider_keeps_its_block_and_patching_is_idempotent(self):
        once = module.patch_compose(TEMPLATE_COMPOSE, ["anthropic"])
        self.assertEqual(once.count("    env_file: ['./.env']"), TEMPLATE_COMPOSE.count("    env_file: ['./.env']"))
        self.assertEqual(module.patch_compose(once, ["anthropic"]), once)
        twice = module.patch_compose(module.patch_compose(TEMPLATE_COMPOSE, ["deepseek"]), ["deepseek"])
        self.assertEqual(twice, module.patch_compose(TEMPLATE_COMPOSE, ["deepseek"]))

    def test_unknown_provider_block_fails(self):
        with self.assertRaises(SystemExit):
            module.patch_compose(TEMPLATE_COMPOSE, ["nonexistent"])

    def test_localize_rewrites_exactly_the_five_local_workers(self):
        patched = module.patch_compose(TEMPLATE_COMPOSE, [], Path("/src/workers"))
        for worker in module.LOCALIZED_WORKERS:
            self.assertIn(f"  {worker}:\n    worker: path:///src/workers/{worker}\n    scripts:\n"
                          f"      run: /src/workers/{worker}/target/release/{worker}\n", patched)
        self.assertNotIn("package://console", patched)
        self.assertIn("package://http", patched)
        self.assertIn("package://harness", TEMPLATE_COMPOSE)
        self.assertNotIn("package://harness", patched)
        self.assertEqual(patched.count("    working_dir: ."), 5)
        self.assertEqual(module.patch_compose(patched, [], Path("/src/workers")), patched)

    def test_localize_fails_when_a_worker_is_missing(self):
        with self.assertRaises(SystemExit):
            module.patch_compose(TEMPLATE_COMPOSE.replace("  harness:", "  harness-x:"), [], Path("/w"))


class CommandTests(unittest.TestCase):
    def test_compose_status_runs_iii_inside_the_project(self):
        seen = {}

        def fake_run(args, **kwargs):
            seen["args"] = args
            seen["cwd"] = kwargs.get("cwd")
            return SimpleNamespace(stdout='{"containers": [], "daemon_pid": 7}')

        with tempfile.TemporaryDirectory() as temp:
            project = Path(temp)
            (project / "worker-compose.yaml").write_text("namespace: default\n")
            with patch.object(module, "run", fake_run):
                status = module.compose_status("iii", module.project_dir(str(project)))
        self.assertEqual(status["daemon_pid"], 7)
        self.assertEqual(seen["cwd"], project.resolve())
        self.assertEqual(seen["args"][:3], ["iii", "trigger", "compose::status"])
        with self.assertRaises(SystemExit):
            module.project_dir(temp)  # removed with the TemporaryDirectory

    def test_scaffold_rejects_an_empty_provider_list_before_touching_the_disk(self):
        with tempfile.TemporaryDirectory() as temp:
            args = SimpleNamespace(dir=temp, name="linkly", provider=" , ", localize=None, iii="iii")
            with patch.object(module, "run") as run:
                with self.assertRaises(SystemExit):
                    module.cmd_scaffold(args)
            run.assert_not_called()
            self.assertFalse((Path(temp) / "linkly").exists())


class EnvPatchTests(unittest.TestCase):
    def test_commented_key_is_written_from_the_environment(self):
        patched = module.patch_env(TEMPLATE_ENV, ["deepseek"], {"DEEPSEEK_API_KEY": "sk-test"})
        self.assertIn("\nDEEPSEEK_API_KEY=sk-test\n", patched)
        self.assertNotIn("# DEEPSEEK_API_KEY", patched)
        self.assertIn("# MOONSHOT_API_KEY=your-moonshot-key", patched)

    def test_live_key_is_replaced_and_missing_key_is_appended(self):
        patched = module.patch_env(TEMPLATE_ENV, ["anthropic", "xai"],
                                   {"ANTHROPIC_API_KEY": "sk-a", "XAI_API_KEY": "sk-x"})
        self.assertIn("ANTHROPIC_API_KEY=sk-a\n", patched)
        self.assertNotIn("your-anthropic-key", patched)
        self.assertTrue(patched.endswith("XAI_API_KEY=sk-x"))

    def test_missing_environment_value_or_unknown_provider_fails(self):
        with self.assertRaises(SystemExit):
            module.patch_env(TEMPLATE_ENV, ["deepseek"], {})
        with self.assertRaises(SystemExit):
            module.patch_env(TEMPLATE_ENV, ["mystery"], {"MYSTERY_API_KEY": "x"})


if __name__ == "__main__":
    unittest.main()
