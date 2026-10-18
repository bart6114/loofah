import json
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parent.parent
FILES = {
    *(
        path.relative_to(ROOT).as_posix()
        for directory in ("apps/server/src", "crates/vault-access/src")
        for path in (ROOT / directory).rglob("*.rs")
        if path.name != "tests.rs" and "tests" not in path.relative_to(ROOT).parts
    ),
    "crates/vault-write/src/access.rs",
} - {"apps/server/src/dispatch.rs"}


def main():
    report = json.loads(Path(sys.argv[1]).read_text())
    files = {}
    for data in report["data"]:
        for file in data["files"]:
            path = Path(file["filename"]).resolve()
            if path.is_relative_to(ROOT):
                relative = path.relative_to(ROOT).as_posix()
                if relative in FILES:
                    files[relative] = file["summary"]["lines"]
    failures = [f"Missing coverage: {path}" for path in sorted(FILES - files.keys())]
    for path, lines in sorted(files.items()):
        print(f"{lines['percent']:6.2f}% {path}")
        if lines["count"] == 0 or lines["percent"] < 85:
            failures.append(f"{path}: requires at least 85% line coverage")
    count = sum(lines["count"] for lines in files.values())
    covered = sum(lines["covered"] for lines in files.values())
    total = 100 * covered / count if count else 0
    print(f"Storage production code: {total:.2f}% ({covered}/{count} lines)")
    if total < 90:
        failures.append("Storage production code requires at least 90% line coverage")
    if failures:
        sys.exit("\n".join(failures))


if __name__ == "__main__":
    main()
