"""Converts the measured fruit fly connectome into the text file Cortex reads.

    uv run python -m cortex_ml.connectome.convert Connectivity_783.parquet fly.csv

The input is the connectivity table published with the whole-brain model
of Shiu et al. (FlyWire, materialisation 783), one row per connected pair
of neurons. Three of its columns are used:

- `Presynaptic_Index`, `Postsynaptic_Index`: the two neurons, numbered in
  the order of the accompanying completeness table.
- `Excitatory x Connectivity`: the number of synapses between them,
  negative when the first neuron is inhibitory.

The output has one `source,target,strength` line per pair, which
`cortex --with brain --connectome fly.csv` loads.
"""

import argparse

import pandas as pd

SOURCE = "Presynaptic_Index"
TARGET = "Postsynaptic_Index"
STRENGTH = "Excitatory x Connectivity"


def convert(parquet_path: str, csv_path: str) -> dict:
    table = pd.read_parquet(parquet_path, columns=[SOURCE, TARGET, STRENGTH])

    table = table[table[STRENGTH] != 0]

    table.to_csv(csv_path, index=False, header=["source", "target", "strength"])

    return {
        "neurons": int(max(table[SOURCE].max(), table[TARGET].max())) + 1,
        "connections": len(table),
        "synapses": int(table[STRENGTH].abs().sum()),
        "inhibitory_share": float((table[STRENGTH] < 0).mean()),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Convert the FlyWire connectivity table for Cortex.")

    parser.add_argument("parquet")
    parser.add_argument("csv")

    args = parser.parse_args()

    summary = convert(args.parquet, args.csv)

    print(
        f"{summary['neurons']} neurons, {summary['connections']} connected pairs, "
        f"{summary['synapses']} synapses, {summary['inhibitory_share']:.0%} of pairs inhibitory"
    )
    print(f"wrote {args.csv}")


if __name__ == "__main__":
    main()
