"""A deterministic byte-pair vocabulary fitted only on local training text."""

import itertools
from collections import Counter


class Tokenizer:
    def __init__(self, tokens=None, merges=None):
        self.tokens = tokens if tokens is not None else [[]] + [[i] for i in range(256)]
        self.merges = merges if merges is not None else []
        self._backend = None

    @staticmethod
    def byte_alphabet():
        # The byte-level alphabet assigns missing bytes sequentially from U+0100.
        missing = [
            b
            for b in range(256)
            if b
            not in list(range(33, 127)) + list(range(161, 173)) + list(range(174, 256))
        ]
        return {
            **{b: chr(b) for b in range(256) if b not in missing},
            **{b: chr(256 + i) for i, b in enumerate(missing)},
        }

    @classmethod
    def fit_fast(cls, texts, size=8192):
        import json

        from tokenizers import Tokenizer as Backend
        from tokenizers import models, pre_tokenizers, trainers

        if not 257 <= size <= 8192:
            raise ValueError("Vocabulary size must be 257..8192")
        backend = Backend(models.BPE())
        backend.pre_tokenizer = pre_tokenizers.ByteLevel(
            add_prefix_space=False, use_regex=False
        )
        backend.train_from_iterator(
            texts,
            trainer=trainers.BpeTrainer(
                vocab_size=size - 1,
                min_frequency=2,
                initial_alphabet=pre_tokenizers.ByteLevel.alphabet(),
                max_token_length=128,
                show_progress=False,
            ),
        )
        alphabet = cls.byte_alphabet()
        lookup = {char: byte + 1 for byte, char in alphabet.items()}
        codec = cls()
        for a, b in json.loads(backend.to_str())["model"]["merges"]:
            pair = [lookup[a], lookup[b]]
            lookup[a + b] = len(codec.tokens)
            codec.merges.append(pair)
            codec.tokens.append(codec.tokens[pair[0]] + codec.tokens[pair[1]])
        return codec

    def fast_backend(self):
        from tokenizers import Tokenizer as Backend
        from tokenizers import models, pre_tokenizers

        if self._backend is None:
            alphabet = self.byte_alphabet()
            symbols = ["".join(alphabet[b] for b in token) for token in self.tokens]
            backend = Backend(
                models.BPE(
                    vocab={s: i for i, s in enumerate(symbols)},
                    merges=[(symbols[a], symbols[b]) for a, b in self.merges],
                )
            )
            backend.pre_tokenizer = pre_tokenizers.ByteLevel(
                add_prefix_space=False, use_regex=False
            )
            self._backend = backend
        return self._backend

    @staticmethod
    def merge(sequence, pair, token):
        pair = tuple(pair)
        result = []
        i = 0
        while i < len(sequence):
            if i + 1 < len(sequence) and (sequence[i], sequence[i + 1]) == pair:
                result.append(token)
                i += 2
            else:
                result.append(sequence[i])
                i += 1
        return result

    @classmethod
    def fit(cls, texts, size=512):
        if not 257 <= size <= 1024:
            raise ValueError("Vocabulary size must be 257..1024")
        tokenizer = cls()
        sequences = [[b + 1 for b in text.encode()] for text in texts]
        while len(tokenizer.tokens) < size:
            counts = Counter(
                pair
                for seq in sequences
                for pair in itertools.pairwise(seq)
                if len(tokenizer.tokens[pair[0]]) + len(tokenizer.tokens[pair[1]])
                <= 128
            )
            if not counts:
                break
            pair, count = counts.most_common(1)[0]
            if count < 2:
                break
            token = len(tokenizer.tokens)
            tokenizer.tokens.append(
                tokenizer.tokens[pair[0]] + tokenizer.tokens[pair[1]]
            )
            tokenizer.merges.append(list(pair))
            sequences = [cls.merge(seq, pair, token) for seq in sequences]
        return tokenizer

    def encode(self, text):
        if len(self.tokens) > 1024:
            return self.fast_backend().encode(text).ids
        sequence = [b + 1 for b in text.encode()]
        for i, pair in enumerate(self.merges):
            sequence = self.merge(sequence, pair, 257 + i)
        return sequence

    def decode(self, tokens):
        return bytes(b for token in tokens if token for b in self.tokens[token]).decode(
            "utf-8", errors="replace"
        )

    def json(self):
        return {"tokens": self.tokens, "merges": self.merges}


def dialogue_tokens(row, tokenizer):
    size = len(tokenizer.tokens)
    question = tokenizer.encode(row["question"])
    answer = tokenizer.encode(row["answer"])
    sequence = [size] + question + [size + 1] + answer + [0]
    target = sequence[1:]
    answer_start = len(question) + 1
    target = [-100 if i < answer_start else token for i, token in enumerate(target)]
    return sequence[:-1], target
