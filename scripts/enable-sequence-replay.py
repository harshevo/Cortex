#!/usr/bin/env python3
"""Attach trusted train/dev replay references without changing tested neural weights."""
import argparse
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

import torch

sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'ml'))
from cortex_ml.circuit.sequence import SequenceDecoder
from cortex_ml.circuit.tokenizer import Tokenizer


def enable(args):
    spec=importlib.util.spec_from_file_location('sequence_training',Path(__file__).with_name('train-learned-instructions.py'))
    training=importlib.util.module_from_spec(spec);spec.loader.exec_module(training)
    checkpoint=json.loads(args.checkpoint.read_text());payload=checkpoint['sequence_decoder']
    model=SequenceDecoder(**payload['settings']);model.load_state_dict({k:torch.tensor(v) for k,v in payload['weights'].items()});model.eval()
    codec=Tokenizer(checkpoint['dialogue']['tokens'],checkpoint['dialogue']['merges']);backend=codec.fast_backend()
    language={s:[] for s in ['train','dev']};monitor={s:[] for s in language}
    for row in json.loads(args.pairs.read_text()):
        if row['split'] in language:language[row['split']].append({**row,'activity':[0.0]*84})
    for row in json.loads(args.states.read_text())['rows']:
        if row['split'] in monitor:monitor[row['split']].append({**row,'activity':row['features'], 'input':backend.encode(row['question']).ids,'target':backend.encode(row['answer']).ids+[0]})
    payload['training'].update(training.replay_metadata(model,language,monitor,'cpu'))
    with args.output.open('x') as stream:json.dump(checkpoint,stream,separators=(',',':'))
    print(json.dumps(dict(metadata_only=True,output=str(args.output),weights_sha256=hashlib.sha256(json.dumps(payload['weights'],sort_keys=True).encode()).hexdigest())))


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ['checkpoint','pairs','states','output']:parser.add_argument('--'+name,type=Path,required=True)
    enable(parser.parse_args())
