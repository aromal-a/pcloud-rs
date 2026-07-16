#!/usr/bin/env python3
"""Generate the exhaustive pcloud-rs architecture atlas catalogs.

Generated files are intentionally source-derived. Do not hand-edit anything
under src/generated/.
"""

from __future__ import annotations

import datetime as dt
import json
import os
import re
import subprocess
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

ATLAS = Path(__file__).resolve().parents[1]
ROOT = ATLAS.parents[1]
SRC = ATLAS / "src"
GENERATED = SRC / "generated"
GITHUB = "https://github.com/ezechiel203/pcloud-rs/blob/main"

FUNCTION_ITEM = re.compile(
    r"^\s*(?P<visibility>pub(?:\([^)]*\))?\s+)?"
    r"(?:(?:async|unsafe|const|default)\s+)*"
    r"(?:extern\s+\"[^\"]+\"\s+)?"
    r"(?P<kind>fn)\s+"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)"
)
OTHER_ITEM = re.compile(
    r"^\s*(?P<visibility>pub(?:\([^)]*\))?\s+)?"
    r"(?P<kind>struct|enum|trait|type|const|static|mod)\s+"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)"
)

STABLE = {"pcloud-sdk"}
INTERNAL = {
    "pcloud-auth",
    "pcloud-backends",
    "pcloud-crypto",
    "pcloud-engine",
    "pcloud-error",
    "pcloud-ipc",
    "pcloud-model",
    "pcloud-observability",
    "pcloud-proto",
    "pcloud-resilience",
    "pcloud-secret",
    "pcloud-store",
}
EVOLVING = {
    "pcloud-cache",
    "pcloud-cli",
    "pcloud-config",
    "pcloud-daemon",
    "pcloud-embedded-sdk",
    "pcloud-fs",
    "pcloud-session",
    "pcloud-web",
}
VERIFY = {"pcloud-chaos", "pcloud-live-e2e", "pcloud-mockserver"}


def run(*args: str) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def cargo_metadata() -> dict[str, Any]:
    return json.loads(
        run("cargo", "metadata", "--format-version", "1", "--no-deps")
    )


def git_files() -> list[str]:
    raw = run(
        "git",
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
    )
    return sorted(path for path in raw.split("\0") if path)


def first_meaningful_line(path: Path) -> str:
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return ""
    suffix = path.suffix.lower()
    if suffix == ".rs":
        for line in lines[:120]:
            stripped = line.strip()
            if stripped.startswith("//!"):
                text = stripped[3:].strip().lstrip("#").strip()
                if text:
                    return text
        for line in lines[:120]:
            stripped = line.strip()
            if stripped.startswith("///"):
                text = stripped[3:].strip().lstrip("#").strip()
                if text:
                    return text
    if suffix in {".md", ".markdown"}:
        for line in lines[:80]:
            if line.startswith("#"):
                return line.lstrip("#").strip()
    for line in lines[:80]:
        stripped = line.strip()
        if not stripped or stripped.startswith(("#!", "<?xml", "<!DOCTYPE")):
            continue
        if stripped.startswith(("#", "//", ";", "<!--")):
            text = stripped.lstrip("#/;<!- ").rstrip("-> ").strip()
            if text:
                return text
    return ""


def compact(text: str, limit: int = 130) -> str:
    text = re.sub(r"\s+", " ", text).strip()
    if len(text) <= limit:
        return text
    return text[: limit - 1].rstrip() + "…"


def file_kind(path: str) -> str:
    p = Path(path)
    name = p.name
    suffix = p.suffix.lower()
    if name == "Cargo.toml":
        return "Cargo manifest"
    if name == "Cargo.lock":
        return "dependency lock"
    if name in {"main.rs", "lib.rs", "build.rs"}:
        return {"main.rs": "binary root", "lib.rs": "library root", "build.rs": "build script"}[name]
    if "/tests/" in f"/{path}" or p.parent.name == "tests":
        return "test"
    if "/benches/" in f"/{path}" or p.parent.name == "benches":
        return "benchmark"
    if "/examples/" in f"/{path}" or p.parent.name == "examples":
        return "example"
    if suffix == ".rs":
        return "Rust module"
    if suffix in {".md", ".markdown", ".rst"}:
        return "documentation"
    if suffix in {".yml", ".yaml"}:
        return "YAML/config"
    if suffix in {".toml", ".json", ".json5", ".ini", ".cfg", ".conf"}:
        return "configuration"
    if suffix in {".sh", ".zsh", ".bash", ".ps1", ".bat", ".cmd"}:
        return "script"
    if suffix in {".wxs", ".nuspec", ".desktop", ".plist", ".service", ".socket"}:
        return "packaging/service"
    if suffix in {".png", ".jpg", ".jpeg", ".gif", ".ico", ".svg"}:
        return "asset"
    if suffix in {".csv"}:
        return "data matrix"
    if name.startswith("Dockerfile"):
        return "container build"
    if name.startswith("."):
        return "project configuration"
    return suffix.lstrip(".") or "file"


def area_for(path: str) -> str:
    top = path.split("/", 1)[0]
    if top == "crates":
        return "crates"
    if top == "vendor":
        return "vendor"
    if top == "packaging":
        return "packaging"
    if top in {".github", ".cargo", "scripts", "tools", "fuzz"}:
        return "automation"
    if top in {"tests", "ops", "deploy"}:
        return "operations-tests"
    if top in {
        "docs",
    }:
        return "documentation"
    if top in {
        ".audits",
        ".audit-fragments",
        "CLAUDEREV",
        "GPTREV",
        ".plans",
    }:
        return "historical"
    if top.startswith(".") and top not in {".env.example", ".envrc"}:
        return "project-meta"
    return "root"


def describe_file(path_text: str) -> str:
    path = ROOT / path_text
    p = Path(path_text)
    kind = file_kind(path_text)
    source = first_meaningful_line(path)
    if source:
        return compact(source)
    if p.name == "Cargo.toml":
        return "Defines package/workspace metadata, features, targets, and dependencies."
    if p.name == "Cargo.lock":
        return "Pins the resolved dependency graph for reproducible workspace builds."
    if p.name == "main.rs":
        return "Executable process entrypoint and top-level lifecycle."
    if p.name == "lib.rs":
        return "Crate root, public exports, and crate-level contract."
    if p.name == "build.rs":
        return "Cargo build-time platform or generated-code integration."
    if kind == "test":
        return "Executable verification for the behavior named by this file."
    if kind == "benchmark":
        return "Performance benchmark for the behavior named by this file."
    if kind == "example":
        return "Runnable usage example."
    if path_text.startswith(".github/workflows/"):
        return "GitHub Actions workflow for the named build, test, release, or qualification gate."
    if path_text.startswith("packaging/"):
        return "Packaging, service lifecycle, installer, or platform-distribution asset."
    if path_text.startswith("vendor/"):
        return "Vendored upstream dependency file; not a pcloud-rs architectural owner."
    if p.suffix == ".rs":
        return f"Rust {p.stem.replace('_', ' ')} module."
    return f"{kind.capitalize()} used by the {area_for(path_text).replace('-', ' ')} area."


def source_link(path: str, line: int | None = None) -> str:
    url = f"{GITHUB}/{path}"
    if line:
        url += f"#L{line}"
    return url


def md_escape(text: str) -> str:
    return (
        text.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("|", "\\|")
        .replace("[", "\\[")
        .replace("]", "\\]")
        .replace("\n", " ")
    )


def maturity(name: str) -> str:
    if name in STABLE:
        return "Stable public contract"
    if name in INTERNAL:
        return "Internal stable"
    if name in EVOLVING:
        return "Evolving product surface"
    if name in VERIFY:
        return "Verification support"
    return "Experimental / bounded"


def crate_files(package: dict[str, Any], files: list[str]) -> list[str]:
    manifest = Path(package["manifest_path"])
    directory = manifest.parent.relative_to(ROOT).as_posix()
    prefix = directory + "/"
    return [path for path in files if path == f"{directory}/Cargo.toml" or path.startswith(prefix)]


def rust_items(path_text: str) -> list[tuple[str, str, str, str, int]]:
    path = ROOT / path_text
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    result: list[tuple[str, str, str, str, int]] = []
    docs: list[str] = []
    for number, line in enumerate(lines, 1):
        stripped = line.strip()
        if stripped.startswith("///"):
            docs.append(stripped[3:].strip())
            continue
        if stripped.startswith("#[") or not stripped:
            continue
        match = FUNCTION_ITEM.match(line) or OTHER_ITEM.match(line)
        if match:
            summary = compact(" ".join(docs), 110) if docs else ""
            visibility = (match.group("visibility") or "private").strip()
            result.append(
                (
                    visibility,
                    match.group("kind"),
                    match.group("name"),
                    summary,
                    number,
                )
            )
        docs = []
    return result


def package_slug(package: dict[str, Any]) -> str:
    return package["name"]


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp-{os.getpid()}")
    temporary.write_text(text.rstrip() + "\n", encoding="utf-8")
    os.replace(temporary, path)


def package_page(package: dict[str, Any], files: list[str]) -> str:
    name = package["name"]
    manifest_rel = Path(package["manifest_path"]).relative_to(ROOT).as_posix()
    directory = str(Path(manifest_rel).parent)
    targets = package.get("targets", [])
    dependencies = sorted(
        {
            dependency.get("rename") or dependency["name"]
            for dependency in package.get("dependencies", [])
        }
    )
    features = package.get("features", {})
    package_files = crate_files(package, files)
    readme_path = ROOT / directory / "README.md"
    description = package.get("description") or (
        first_meaningful_line(readme_path) if readme_path.exists() else ""
    )
    if not description:
        description = first_meaningful_line(ROOT / directory / "src/lib.rs")
    lines = [
        f"# `{name}`",
        "",
        f"**Maturity:** {maturity(name)}",
        "",
        f"**Version:** `{package['version']}`",
        "",
        f"**Directory:** `{directory}`",
        "",
        f"**Manifest:** [`{manifest_rel}`]({source_link(manifest_rel)})",
        "",
        md_escape(description or "Cargo workspace package.") ,
        "",
        "## Targets",
        "",
        "| Cargo target | Kinds | Source |",
        "|---|---|---|",
    ]
    for target in targets:
        src = Path(target["src_path"]).relative_to(ROOT).as_posix()
        kinds = ", ".join(target.get("kind", []))
        lines.append(
            f"| `{target['name']}` | {kinds} | [`{src}`]({source_link(src)}) |"
        )
    lines += [
        "",
        "## Direct dependencies",
        "",
        ", ".join(f"`{name}`" for name in dependencies) if dependencies else "None.",
        "",
        "## Cargo features",
        "",
    ]
    if features:
        lines += [
            "| Feature | Enables |",
            "|---|---|",
        ]
        for feature, values in sorted(features.items()):
            value_text = ", ".join(f"`{value}`" for value in values) or "empty marker"
            lines.append(f"| `{feature}` | {value_text} |")
    else:
        lines.append("No declared package features.")
    lines += [
        "",
        f"## File inventory ({len(package_files)})",
        "",
        "| File | Kind | Role |",
        "|---|---|---|",
    ]
    for path in package_files:
        lines.append(
            f"| [`{path}`]({source_link(path)}) | {file_kind(path)} | "
            f"{md_escape(describe_file(path))} |"
        )
    symbols: list[tuple[str, str, str, str, int, str]] = []
    for path in package_files:
        if path.endswith(".rs"):
            for visibility, kind, symbol, doc, line in rust_items(path):
                symbols.append((path, visibility, kind, symbol, line, doc))
    public_count = sum(
        1 for _, visibility, _, _, _, _ in symbols if visibility.startswith("pub")
    )
    lines += [
        "",
        f"## Rust declaration index ({len(symbols)} total; {public_count} visible)",
        "",
    ]
    if symbols:
        lines += [
            "| Item | Visibility | Kind | Source | Documentation hint |",
            "|---|---|---|---|---|",
        ]
        for path, visibility, kind, symbol, line, doc in symbols:
            lines.append(
                f"| `{symbol}` | `{visibility}` | {kind} | "
                f"[`{path}:{line}`]({source_link(path, line)}) | "
                f"{md_escape(doc or 'Read the source/rustdoc for the exact contract.')} |"
            )
    else:
        lines.append(
            "No named Rust declarations were found. The package may be manifest-only "
            "or rely on generated source."
        )
    lines += [
        "",
        "## Usage guidance",
        "",
    ]
    label = maturity(name)
    if label == "Stable public contract":
        lines.append(
            "This is the intended third-party SemVer boundary. The daemon must be "
            "running and authenticated; registry release qualification is tracked separately."
        )
    elif label == "Internal stable":
        lines.append(
            "Core workspace code may depend on this contract. External applications should "
            "prefer `pcloud-sdk` unless they intentionally own the lower-level runtime."
        )
    elif label == "Verification support":
        lines.append(
            "This package proves behavior and is not a shipped end-user runtime surface."
        )
    elif label == "Evolving product surface":
        lines.append(
            "This is product code but not a frozen external library contract. Check current "
            "status and native qualification before deployment claims."
        )
    else:
        lines.append(
            "Treat this package as experimental, optional, enterprise-bounded, or unshipped "
            "until its feature and release evidence says otherwise."
        )
    return "\n".join(lines)


AREA_TITLES = {
    "root": "Root product and policy files",
    "crates": "Workspace crate files",
    "documentation": "Documentation files",
    "packaging": "Packaging and service files",
    "automation": "Automation, workflows, scripts, and fuzz files",
    "operations-tests": "Operations, deployment, tools, and cross-crate tests",
    "historical": "Historical audits, plans, and review evidence",
    "project-meta": "Project metadata and local development definitions",
    "vendor": "Vendored upstream files",
}


def inventory_page(area: str, paths: list[str]) -> str:
    kinds = Counter(file_kind(path) for path in paths)
    lines = [
        f"# {AREA_TITLES[area]}",
        "",
        f"This generated page covers **{len(paths)}** Git-visible files.",
        "",
        "Kind summary: "
        + ", ".join(f"{kind}: {count}" for kind, count in kinds.most_common()),
        "",
    ]
    if area == "vendor":
        lines += [
            "> Vendored files are upstream implementation details. They are listed for "
            "exhaustiveness but are not pcloud-rs-owned entrypoints.",
            "",
        ]
    if area == "historical":
        lines += [
            "> Historical reports describe past snapshots. Prefer current source, tests, "
            "`STATUS.md`, and release evidence for present-tense claims.",
            "",
        ]
    lines += [
        "| File | Kind | Source-derived role |",
        "|---|---|---|",
    ]
    for path in paths:
        lines.append(
            f"| [`{path}`]({source_link(path)}) | {file_kind(path)} | "
            f"{md_escape(describe_file(path))} |"
        )
    return "\n".join(lines)


def crate_index(packages: list[dict[str, Any]], files: list[str]) -> str:
    lines = [
        "# Workspace crate catalog",
        "",
        f"Cargo currently reports **{len(packages)} packages**.",
        "",
        "| Package | Version | Maturity | Targets | Files | Directory |",
        "|---|---:|---|---|---:|---|",
    ]
    for package in sorted(packages, key=lambda item: item["name"]):
        directory = Path(package["manifest_path"]).parent.relative_to(ROOT).as_posix()
        kinds = sorted(
            {
                kind
                for target in package.get("targets", [])
                for kind in target.get("kind", [])
            }
        )
        count = len(crate_files(package, files))
        slug = package_slug(package)
        lines.append(
            f"| [`{package['name']}`](./{slug}.md) | `{package['version']}` | "
            f"{maturity(package['name'])} | {', '.join(kinds)} | {count} | "
            f"`{directory}` |"
        )
    lines += [
        "",
        "## Dependency overview",
        "",
        "Each package page lists its direct Cargo dependencies. Read arrows as "
        "“uses”; this schematic highlights the primary product path rather than "
        "every optional/test edge.",
        "",
        "```text",
        "pcloud-cli ─────────────► pcloud-ipc ─────► pcloud-model",
        "pcloud-sdk ─────────────► pcloud-ipc",
        "pcloud-web / webdav* ───► pcloud-ipc",
        "                                  ▲",
        "                                  │",
        "pcloud-daemon ─► pcloud-backends ─┼─► pcloud-proto ─► TLS/pCloud",
        "       │              │           │",
        "       │              ├───────────┴─► pcloud-store / cache",
        "       ├──────────────► pcloud-engine",
        "       ├──────────────► pcloud-fs",
        "       ├──────────────► pcloud-crypto / auth / secret",
        "       └──────────────► observability / resilience / policy",
        "",
        "* experimental/unshipped where documented",
        "```",
    ]
    return "\n".join(lines)


def workspace_snapshot(files: list[str], packages: list[dict[str, Any]]) -> str:
    head = run("git", "rev-parse", "HEAD")
    dirty = bool(run("git", "status", "--porcelain"))
    timestamp = dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()
    counts = Counter(area_for(path) for path in files)
    lines = [
        "# Generated workspace snapshot",
        "",
        f"- Generated: `{timestamp}`",
        f"- Git HEAD: `{head}`",
        f"- Worktree: `{'dirty' if dirty else 'clean'}`",
        f"- Cargo packages: **{len(packages)}**",
        f"- Git-visible files inventoried: **{len(files)}**",
        "",
        "## File coverage",
        "",
        "| Area | Files | Inventory |",
        "|---|---:|---|",
    ]
    for area in AREA_TITLES:
        lines.append(
            f"| {AREA_TITLES[area]} | {counts.get(area, 0)} | "
            f"[open](inventory/{area}.md) |"
        )
    lines += [
        "",
        "> A dirty snapshot is useful for development navigation but is not a "
        "reproducible release baseline.",
    ]
    return "\n".join(lines)


def summary(packages: list[dict[str, Any]]) -> str:
    lines = [
        "# Summary",
        "",
        "- [Architecture Atlas](./index.md)",
        "- [Truth, maturity, and scope](./truth-and-scope.md)",
        "",
        "# Architecture",
        "",
        "- [System overview](./system-overview.md)",
        "- [RemoteFs canonical boundary](./remote-fs.md)",
        "- [Entrypoints and public surfaces](./entrypoints.md)",
        "- [Request and data paths](./request-paths.md)",
        "- [State, transfers, and durability](./storage-durability.md)",
        "- [Security and trust boundaries](./security-boundaries.md)",
        "",
        "# Use and operations",
        "",
        "- [Standalone and library use](./standalone-library.md)",
        "- [Operations and platforms](./operations-platforms.md)",
        "- [Developer and extension guide](./developer-guide.md)",
        "- [Verification and evidence](./verification.md)",
        "",
        "# Generated source reference",
        "",
        "- [Workspace snapshot](./generated/snapshot.md)",
        "- [Workspace crate catalog](./generated/crates/index.md)",
    ]
    for package in sorted(packages, key=lambda item: item["name"]):
        lines.append(
            f"  - [`{package['name']}`](./generated/crates/{package_slug(package)}.md)"
        )
    lines += [
        "- [File inventory methodology](./inventory-methodology.md)",
        "- [Complete file inventory](./generated/inventory/index.md)",
    ]
    for area, title in AREA_TITLES.items():
        lines.append(f"  - [{title}](./generated/inventory/{area}.md)")
    return "\n".join(lines)


def inventory_index(files: list[str]) -> str:
    by_area: dict[str, list[str]] = defaultdict(list)
    for path in files:
        by_area[area_for(path)].append(path)
    lines = [
        "# Complete project file inventory",
        "",
        f"**{len(files)} tracked or unignored working-tree files** are covered.",
        "",
        "| Area | Files | Page |",
        "|---|---:|---|",
    ]
    for area, title in AREA_TITLES.items():
        lines.append(
            f"| {title} | {len(by_area.get(area, []))} | "
            f"[open](./{area}.md) |"
        )
    lines += [
        "",
        "The inventory includes vendored upstream files for exhaustiveness and "
        "labels them separately. Ignored build/runtime output is excluded.",
    ]
    return "\n".join(lines)


def main() -> int:
    metadata = cargo_metadata()
    packages = metadata["packages"]
    # Read the prior generated tree so the inventory is self-describing.
    files = git_files()
    GENERATED.mkdir(parents=True, exist_ok=True)

    write(GENERATED / "snapshot.md", workspace_snapshot(files, packages))
    write(GENERATED / "crates/index.md", crate_index(packages, files))
    for package in packages:
        write(
            GENERATED / "crates" / f"{package_slug(package)}.md",
            package_page(package, files),
        )

    by_area: dict[str, list[str]] = defaultdict(list)
    for path in files:
        by_area[area_for(path)].append(path)
    write(GENERATED / "inventory/index.md", inventory_index(files))
    for area in AREA_TITLES:
        write(
            GENERATED / "inventory" / f"{area}.md",
            inventory_page(area, by_area.get(area, [])),
        )
    write(SRC / "SUMMARY.md", summary(packages))

    print(
        f"architecture atlas: generated {len(packages)} package pages and "
        f"inventoried {len(files)} files"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
