#!/usr/bin/env python3
"""Fail when the architecture atlas stops covering a declared feature surface."""

from __future__ import annotations

import csv
import sys
from pathlib import Path

import generate


ATLAS = Path(__file__).resolve().parents[1]
SRC = ATLAS / "src"
GENERATED_FEATURES = SRC / "generated" / "features"

CURATED = (
    "features/index.md",
    "features/personal-cloud.md",
    "features/sync-mount-transfer.md",
    "features/crypto.md",
    "features/collaboration-enterprise.md",
    "features/interfaces-automation.md",
    "features/runtime-internals.md",
    "features/platform-operations.md",
    "features/verification-helpers.md",
)

GENERATED = (
    "generated/features/package-families.md",
    "generated/features/api-capabilities.md",
    "generated/features/current-surfaces.md",
    "generated/features/cargo-flags.md",
    "generated/features/source-units.md",
)


def main() -> int:
    failures: list[str] = []
    metadata = generate.cargo_metadata()
    packages = metadata["packages"]
    package_names = {package["name"] for package in packages}
    profile_names = set(generate.CRATE_PROFILES)

    if missing := sorted(package_names - profile_names):
        failures.append(f"packages without feature profiles: {', '.join(missing)}")
    if stale := sorted(profile_names - package_names):
        failures.append(f"stale package feature profiles: {', '.join(stale)}")

    flags = {
        (package["name"], feature)
        for package in packages
        for feature in package.get("features", {})
    }
    guided_flags = set(generate.FEATURE_FLAG_GUIDANCE)
    if missing := sorted(flags - guided_flags):
        failures.append(
            "feature flags without explicit rationale: "
            + ", ".join(f"{package}/{feature}" for package, feature in missing)
        )
    if stale := sorted(guided_flags - flags):
        failures.append(
            "stale feature-flag guidance: "
            + ", ".join(f"{package}/{feature}" for package, feature in stale)
        )

    matrix = generate.ROOT / "C_FEATURE_PARITY_MATRIX.csv"
    with matrix.open(newline="", encoding="utf-8-sig") as handle:
        capability_rows = list(csv.DictReader(handle))
    capability_page = GENERATED_FEATURES / "api-capabilities.md"
    if capability_page.exists():
        rendered = capability_page.read_text(encoding="utf-8")
        expected = f"Coverage: {len(capability_rows)} of {len(capability_rows)} matrix rows"
        if expected not in rendered:
            failures.append(f"API catalog does not report {expected!r}")
        rendered_rows = rendered.count('<span class="atlas-supported">') + rendered.count(
            '<span class="atlas-experimental">'
        )
        if rendered_rows != len(capability_rows):
            failures.append(
                f"API catalog renders {rendered_rows} status rows; expected {len(capability_rows)}"
            )
    else:
        failures.append(f"missing generated page: {capability_page.relative_to(ATLAS)}")

    commands = generate.rust_enum_variants(
        "crates/pcloud-cli/src/commands.rs", "Command"
    )
    command_routes = generate.command_routes({name for name, _, _ in commands})
    for name, _, _ in commands:
        request_names, method_names, local_only = command_routes[name]
        if not request_names and not method_names and not local_only:
            failures.append(f"CLI command has no extracted IPC/local route: {name}")
    methods = generate.rust_enum_variants(
        "crates/pcloud-ipc/src/methods.rs", "Method"
    )
    requests = generate.rust_enum_variants(
        "crates/pcloud-ipc/src/methods.rs", "Request"
    )
    binaries = [
        target["name"]
        for package in packages
        for target in package.get("targets", [])
        if "bin" in target.get("kind", [])
    ]
    surface_page = GENERATED_FEATURES / "current-surfaces.md"
    if surface_page.exists():
        rendered = surface_page.read_text(encoding="utf-8")
        expected = (
            f"Coverage: {len(commands)} of {len(commands)} CLI commands, "
            f"{len(methods)} of {len(methods)} argumentless IPC methods, "
            f"{len(requests)} of {len(requests)} argument-bearing IPC requests, and "
            f"{len(binaries)} of {len(binaries)} Cargo binaries"
        )
        if expected not in rendered:
            failures.append(f"current-surface catalog does not report {expected!r}")
        for kind, variants in (
            ("Command", commands),
            ("Method", methods),
            ("Request", requests),
        ):
            for name, _, _ in variants:
                if f"[`{name}`]" not in rendered:
                    failures.append(f"current-surface catalog omits {kind}::{name}")
        for name in binaries:
            if f"`{name}`" not in rendered:
                failures.append(f"current-surface catalog omits binary {name}")
    else:
        failures.append(f"missing generated page: {surface_page.relative_to(ATLAS)}")

    files = generate.git_files()
    source_units = sum(
        1
        for package in packages
        for path in generate.crate_files(package, files)
        if path.endswith(".rs")
    )
    source_page = GENERATED_FEATURES / "source-units.md"
    if source_page.exists():
        rendered = source_page.read_text(encoding="utf-8")
        expected = f"Coverage: {source_units} of {source_units} Rust source/test/helper files"
        if expected not in rendered:
            failures.append(f"source-unit catalog does not report {expected!r}")
    else:
        failures.append(f"missing generated page: {source_page.relative_to(ATLAS)}")

    inventory_page = SRC / "generated" / "inventory" / "index.md"
    if inventory_page.exists():
        rendered = inventory_page.read_text(encoding="utf-8")
        expected = f"**{len(files)} tracked or unignored working-tree files**"
        if expected not in rendered:
            failures.append(f"file inventory does not report {expected!r}")
    else:
        failures.append(f"missing generated page: {inventory_page.relative_to(ATLAS)}")

    summary = (SRC / "SUMMARY.md").read_text(encoding="utf-8")
    for relative in CURATED + GENERATED:
        if not (SRC / relative).exists():
            failures.append(f"missing feature chapter: {relative}")
        summary_target = f"./{relative}"
        if summary_target not in summary:
            failures.append(f"feature chapter absent from SUMMARY.md: {relative}")

    platform_page = (SRC / "features" / "platform-operations.md").read_text(
        encoding="utf-8"
    )
    for platform in generate.REQUIRED_PLATFORMS:
        if platform not in platform_page:
            failures.append(f"platform lifecycle/reference missing {platform}")
    verification_page = (SRC / "features" / "verification-helpers.md").read_text(
        encoding="utf-8"
    )
    for category in generate.VERIFICATION_CATEGORIES:
        if category.casefold() not in verification_page.casefold():
            failures.append(f"verification chapter missing category {category}")

    for markdown in (SRC / "generated").rglob("*.md"):
        raw = markdown.read_bytes()
        if b"\x00" in raw:
            failures.append(
                f"generated Markdown contains a NUL byte: {markdown.relative_to(ATLAS)}"
            )
        if b"github.com/ezechiel203/pcloud-rs/blob/main/.pcloud-rust-dev/" in raw:
            failures.append(
                f"generated Markdown links to credential/runtime state: {markdown.relative_to(ATLAS)}"
            )

    if failures:
        print("Feature coverage failures:", file=sys.stderr)
        print("\n".join(f"- {failure}" for failure in failures), file=sys.stderr)
        return 1

    print(
        "atlas feature coverage: "
        f"{len(packages)} packages, {len(flags)} Cargo flags, "
        f"{len(capability_rows)} API decisions, {len(commands)} CLI commands, "
        f"{len(methods)} IPC methods, {len(requests)} IPC requests, "
        f"{len(binaries)} binaries, {source_units} Rust units, "
        f"{len(files)} project files, "
        f"{len(CURATED) + len(GENERATED)} feature chapters OK"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
