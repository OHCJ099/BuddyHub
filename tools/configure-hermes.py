"""Pin Hermes to regional WorkBuddy 4.1 routes and native reasoning controls."""
import argparse
import datetime
import json
import os
from pathlib import Path
import re
import shutil
import yaml

MODELS = ["deepseek-v4.1-flash-cn", "deepseek-v4.1-flash-ai", "deepseek-v4.1-flash-auto"]

def configure(home: Path):
    path = home / "config.yaml"
    cfg = yaml.safe_load(path.read_text(encoding="utf-8-sig"))
    provider = cfg.get("providers", {}).get("workbuddy")
    if not isinstance(provider, dict):
        raise ValueError(f"{home}: 未找到 WorkBuddy 配置")
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    backup = home / "backups" / f"buddyhub-bridge-{stamp}"
    backup.mkdir(parents=True, exist_ok=True)
    for name in ["config.yaml", ".env", "provider_models_cache.json"]:
        source = home / name
        if source.exists():
            shutil.copy2(source, backup / name)
    existing = provider.get("models", {})
    provider["models"] = {name: {**(existing.get(name, {}) if isinstance(existing, dict) and isinstance(existing.get(name, {}), dict) else {}), "supports_vision": True} for name in MODELS}
    provider["session_affinity_header"] = "X-BuddyHub-Session-ID"
    provider["discover_models"] = False
    provider.pop("models_discovered", None)
    extra = provider.get("extra_body", {})
    if isinstance(extra, dict):
        extra.pop("reasoning_effort", None)
        extra.pop("reasoning", None)
        if not extra:
            provider.pop("extra_body", None)
    model = cfg.get("model", {})
    if isinstance(model, dict) and "workbuddy" in str(model.get("provider", "")).lower():
        current = str(model.get("default", ""))
        region = "auto" if "-auto" in current else "cn" if "-cn" in current else "ai"
        model["default"] = f"deepseek-v4.1-flash-{region}"
        model["api_mode"] = "chat_completions"
    path.write_text(yaml.safe_dump(cfg, allow_unicode=True, sort_keys=False), encoding="utf-8")
    buddy = json.loads((Path(os.environ["USERPROFILE"]) / ".buddyhub" / "config.json").read_text(encoding="utf-8-sig"))
    env_path = home / ".env"
    env_text = env_path.read_text(encoding="utf-8-sig") if env_path.exists() else ""
    key_env = provider.get("key_env", "WORKBUDDY_API_KEY")
    line = f"{key_env}={buddy['localKey']}"
    pattern = rf"^{re.escape(key_env)}\s*=.*$"
    env_text = re.sub(pattern, lambda match: line, env_text, flags=re.MULTILINE) if re.search(pattern, env_text, re.MULTILINE) else env_text.rstrip() + "\n" + line + "\n"
    env_path.write_text(env_text, encoding="utf-8")
    # Discovery is disabled; only remove this endpoint's stale catalog, preserving others.
    cache_path = home / "provider_models_cache.json"
    if cache_path.exists():
        cache = json.loads(cache_path.read_text(encoding="utf-8-sig"))
        cache = {key: value for key, value in cache.items()
                 if "127.0.0.1:8787" not in key and "localhost:8787" not in key
                 and "workbuddy" not in key.lower()}
        cache_path.write_text(json.dumps(cache, ensure_ascii=False), encoding="utf-8")
    print(f"{home}: 已配置中国版、国际版、双区轮询三个 4.1 入口")

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("homes", nargs="+", type=Path)
    args = parser.parse_args()
    for home in args.homes:
        configure(home)
