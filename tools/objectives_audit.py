#!/usr/bin/env python3
"""Run the CPU Quick Mission placement audit and retain complete finding tables.

Uses the user's isolated TORE_DATA_DIR. Does not build or open windows.
"""
import argparse
import csv
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/tore-app"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--seeds", type=int, default=3)
    parser.add_argument("--fast", action="store_true")
    parser.add_argument("--uncached", action="store_true", help="rebuild every scene instead of reusing identical object layouts")
    parser.add_argument("--theater", action="append", default=[])
    parser.add_argument("--supplemental", action="store_true", help="also run final mission starts, grounding and airport audits")
    args = parser.parse_args()
    if args.seeds < 1:
        parser.error("--seeds must be positive")
    args.output.mkdir(parents=True, exist_ok=True)
    common = ["--seeds", str(args.seeds), *(["--fast"] if args.fast else []), *args.theater]
    commands = [("surface", ["--surface-dump", "--objectives", *common, *(["--uncached"] if args.uncached else [])])]
    if args.supplemental:
        commands += [
            ("starts", ["--surface-dump", "--objective-starts", *common]),
            ("grounding", ["--surface-dump", "--grounding-sites"]),
            ("airports", ["--airfield-audit", *args.theater]),
            ("parked", ["--airfield-audit", "--parked", *args.theater]),
            ("merged", ["--airfield-audit", "--merged"]),
        ]
    statuses = []
    with (args.output / "findings.tsv").open("w", newline="") as table:
        writer = csv.writer(table, delimiter="\t")
        writer.writerow(["kind", "theater", "separation_nm", "template", "aaa", "sam", "seed", "object", "x_ft", "y_ft", "z_ft", "rule"])
        for name, command in commands:
            log = args.output / f"{name}.log"
            print(f"Running {name}: {log}", flush=True)
            with log.open("w") as stream:
                result = subprocess.run([str(args.binary.resolve()), *command], stdout=stream, stderr=subprocess.STDOUT, check=False)
            statuses.append((name, result.returncode))
            with log.open() as stream:
                for line in stream:
                    if line.startswith(("finding\t", "start-finding\t")) and not line.startswith("finding\ttheater\t"):
                        writer.writerow(line.rstrip("\n").split("\t"))
            table.flush()
    (args.output / "status.tsv").write_text("command\texit_code\n" + "".join(f"{name}\t{status}\n" for name, status in statuses))
    return int(any(status for _, status in statuses))


if __name__ == "__main__":
    raise SystemExit(main())
