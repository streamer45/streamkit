# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

from pathlib import Path
import re
import sys


WORKFLOW_PATH = Path(__file__).resolve().parents[1] / ".github/workflows/docker.yml"
EXPECTED_JOBS = {
    "build-cpu": "/mnt/docker-cache/cpu",
    "build-gpu": "/mnt/docker-cache/gpu",
    "build-demo": "/mnt/docker-cache/demo",
}


def extract_job_blocks(workflow: str) -> dict[str, str]:
    pattern = re.compile(
        r"(?ms)^  (?P<job>build-cpu|build-gpu|build-demo):\n"
        r"(?P<body>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)"
    )
    return {match.group("job"): match.group("body") for match in pattern.finditer(workflow)}


def check_job(job: str, cache_path: str, block: str) -> list[str]:
    checks = {
        "setup-buildx step id": "id: buildx",
        "explicit builder": "builder: ${{ steps.buildx.outputs.name }}",
        "cache import": f"cache-from: type=local,src={cache_path}",
        "cache export": f"cache-to: type=local,dest={cache_path},mode=max",
        "cache directory preparation": f"mkdir -p {cache_path}",
    }
    return [f"{job}: missing {name}" for name, value in checks.items() if value not in block]


def main() -> int:
    workflow = WORKFLOW_PATH.read_text()
    blocks = extract_job_blocks(workflow)
    errors = [
        f"{job}: job block not found"
        for job in EXPECTED_JOBS
        if job not in blocks
    ]
    for job, cache_path in EXPECTED_JOBS.items():
        if job in blocks:
            errors.extend(check_job(job, cache_path, blocks[job]))

    if errors:
        print("Docker workflow validation failed:", file=sys.stderr)
        print("\n".join(f"- {error}" for error in errors), file=sys.stderr)
        return 1

    print("Docker workflow validation passed for build-cpu, build-gpu, and build-demo.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
