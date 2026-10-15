from __future__ import annotations

import hashlib
import json
import re
from functools import lru_cache
from pathlib import Path

import torch


def hash_channel(text: str, inputs: int):
    value = 0xCBF29CE484222325
    for byte in text.encode():
        value = ((value ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return value % inputs


def word_channels(text: str, inputs: int):
    words = re.findall(r"[^\W\d_]+", text.lower(), flags=re.UNICODE)
    return sorted(
        {hash_channel(w, inputs) for w in words}
        | {hash_channel("operator:" + s, inputs) for s in text if s in "+-*/=×÷"}
    )


def word_spikes(texts: list[str], config: dict):
    ticks = round(80 / config["dt_ms"])
    interval = round(4 / config["dt_ms"])
    result = torch.zeros(len(texts), ticks, config["inputs"])
    for i, text in enumerate(texts):
        result[i, ::interval, word_channels(text, config["inputs"])] = 1
    return result


def token_channels(token: int, vocab: int):
    if token >= vocab:
        return list(range(160, 224)) + [256 + token - vocab]
    bits, replicas = (10, 8) if vocab <= 1024 else (13, 6)
    return [
        replica * bits * 2 + bit * 2 + ((token >> bit) & 1)
        for replica in range(replicas)
        for bit in range(bits)
    ]


@lru_cache(maxsize=4)
def token_table(vocab: int, inputs: int):
    if not 257 <= vocab <= 8192 or inputs < 260:
        raise ValueError("Token sensor requires vocabulary 257..8192 and >=260 inputs")
    table = torch.zeros(vocab + 3, inputs)
    for token in range(vocab + 3):
        table[token, token_channels(token, vocab)] = 1
    return table


def token_spikes(tokens: torch.Tensor, inputs: int, ticks_per_symbol: int, vocab: int):
    if inputs < 260 or not 1 <= ticks_per_symbol <= 16:
        raise ValueError("Invalid token sensor dimensions")
    spikes = token_table(vocab, inputs)[tokens]
    result = spikes[:, :, None, :].expand(-1, -1, ticks_per_symbol, -1).clone()
    if ticks_per_symbol > 1:
        result[:, :, 1::2] = 0
    return result.flatten(1, 2)


def operation_batch(rows, tokenizer, config, ticks):
    size = len(tokenizer.tokens)
    sequences = [
        [size]
        + tokenizer.encode(re.sub(r"\d+(?:,\d{3})*(?:\.\d+)?", "#", row["question"]))
        + [size + 1]
        for row in rows
    ]
    # Preserve both ends; never use an answer to construct sensory inputs.
    sequences = [seq if len(seq) <= 128 else seq[:64] + seq[-64:] for seq in sequences]
    lengths = torch.tensor([len(seq) for seq in sequences])
    tokens = torch.zeros(len(rows), max(map(len, sequences)), dtype=torch.long)
    for i, seq in enumerate(sequences):
        tokens[i, : len(seq)] = torch.tensor(seq)
    return token_spikes(tokens, config["inputs"], ticks, size), lengths


def dialogue_batch(rows, tokenizer, config, ticks):
    from .tokenizer import dialogue_tokens

    encoded = [dialogue_tokens(row, tokenizer) for row in rows]
    length = max(len(seq) for seq, _ in encoded)
    tokens = torch.zeros(len(rows), length, dtype=torch.long)
    targets = torch.full_like(tokens, -100)
    for i, (seq, gold) in enumerate(encoded):
        tokens[i, : len(seq)] = torch.tensor(seq)
        targets[i, : len(gold)] = torch.tensor(gold)
    return token_spikes(tokens, config["inputs"], ticks, len(tokenizer.tokens)), targets


def lessons(path: Path):
    with path.open() as stream:
        result = [json.loads(line) for line in stream if line.strip()]
    if len(result) > 16_000:
        raise ValueError("At most 16000 rows per split")
    return result


def sha256(path: Path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()
