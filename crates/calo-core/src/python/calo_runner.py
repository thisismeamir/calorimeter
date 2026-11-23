#!/usr/bin/env python3
"""Out-of-process Python service and plot runner for Calorimeter (spec §12.4)."""

import argparse
import csv
import importlib
import sys


def parse_typed_csv(path):
    with open(path, "r", newline="", encoding="utf-8") as f:
        reader = csv.reader(f)
        header = next(reader)
        cols = []
        for h in header:
            if ":" in h:
                name, ty = h.split(":", 1)
            else:
                name, ty = h, "str"
            cols.append((name, ty))
        data = {name: [] for name, _ in cols}
        for row in reader:
            for (name, ty), raw in zip(cols, row):
                if ty == "int":
                    data[name].append(int(raw))
                elif ty == "float":
                    data[name].append(float(raw))
                elif ty == "bool":
                    data[name].append(raw.lower() == "true")
                else:
                    data[name].append(raw)
        return cols, data


def write_typed_csv(path, out_dict):
    cols = list(out_dict.keys())
    n_rows = len(out_dict[cols[0]]) if cols else 0
    headers = []
    for c in cols:
        sample = out_dict[c][0] if n_rows > 0 else ""
        if isinstance(sample, bool):
            ty = "bool"
        elif isinstance(sample, int):
            ty = "int"
        elif isinstance(sample, float):
            ty = "float"
        else:
            ty = "str"
        headers.append(f"{c}:{ty}")
    with open(path, "w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f)
        writer.writerow(headers)
        for i in range(n_rows):
            writer.writerow([out_dict[c][i] for c in cols])


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--module", required=True)
    p.add_argument("--function", required=True)
    p.add_argument("--in", dest="in_csv", required=True)
    p.add_argument("--out", dest="out_csv")
    p.add_argument("--svg", dest="out_svg")
    p.add_argument("--opts", required=True)
    args = p.parse_args()

    mod = importlib.import_module(args.module)
    fn = getattr(mod, args.function)
    _, data = parse_typed_csv(args.in_csv)

    opts = {}
    with open(args.opts, "r", encoding="utf-8") as f:
        for line in f:
            if ":" in line:
                k, v = line.split(":", 1)
                opts[k.strip()] = v.strip()

    if args.out_svg:
        svg_text = fn(data, opts)
        with open(args.out_svg, "w", encoding="utf-8") as f:
            f.write(svg_text)
    elif args.out_csv:
        res = fn(data, opts)
        if "run_id" in data and "run_id" not in res:
            res["run_id"] = data["run_id"]
        write_typed_csv(args.out_csv, res)
    else:
        sys.stderr.write("either --out or --svg must be specified\n")
        sys.exit(2)


if __name__ == "__main__":
    main()
