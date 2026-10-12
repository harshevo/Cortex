import pandas as pd

from cortex_ml.connectome.convert import SOURCE, STRENGTH, TARGET, convert


def test_the_connectivity_table_becomes_synapse_lines(tmp_path):
    table = pd.DataFrame(
        {
            SOURCE: [0, 0, 2, 1],
            TARGET: [1, 2, 0, 2],
            STRENGTH: [5, -3, 12, 0],
            "Connectivity": [5, 3, 12, 0],
        }
    )

    parquet = tmp_path / "connectivity.parquet"
    csv = tmp_path / "fly.csv"

    table.to_parquet(parquet)

    summary = convert(str(parquet), str(csv))

    assert summary == {
        "neurons": 3,
        "connections": 3,
        "synapses": 20,
        "inhibitory_share": 1 / 3,
    }

    assert csv.read_text().splitlines() == ["source,target,strength", "0,1,5", "0,2,-3", "2,0,12"]
