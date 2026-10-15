#!/usr/bin/env python3
"""Held-out generation and state-reporting evaluation; never trains or selects weights."""
import argparse
import collections
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

import numpy as np
import torch

sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'ml'))
from cortex_ml.circuit.sequence import SequenceDecoder,batch
from cortex_ml.circuit.tokenizer import Tokenizer

spec=importlib.util.spec_from_file_location('learned_training',Path(__file__).with_name('train-learned-instructions.py'))
training=importlib.util.module_from_spec(spec);spec.loader.exec_module(training)


def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()


def monitor_metrics(rows):
    positives=[r for r in rows if r['injected']];negatives=[r for r in rows if not r['injected']]
    def correct(r):return r['generated']==r['answer']
    group=collections.defaultdict(list)
    for r in rows:group[r['group']].append(float(correct(r)))
    scores=np.array([np.mean(v) for v in group.values()]);rng=np.random.default_rng(1731)
    interval=np.quantile(rng.choice(scores,(2000,len(scores))).mean(1),[.025,.975]).tolist()
    return dict(trials=len(rows),groups=len(group),balanced_accuracy=(np.mean([correct(r) for r in positives])+np.mean([correct(r) for r in negatives]))/2,
        hit_rate=np.mean([correct(r) for r in positives]),false_alarm_rate=np.mean([r['generated']==('A' if r['yes_is_A'] else 'B') for r in negatives]),
        noncompliance_rate=np.mean([r['generated'] not in ['A','B'] for r in rows]),
        brier_score=np.mean([(r['yes_probability']-r['injected'])**2 for r in rows]),
        descriptive_group_bootstrap_interval=interval)


@torch.no_grad()
def predict(model,rows,codec,device,maximum):
    output=[]
    width=32 if model.settings['hidden']<=256 else 2
    for start in range(0,len(rows),width):
        part=rows[start:start+width];x,n,p,a,y=batch(part,len(codec.tokens),device)
        decoded=model.generate(x,n,a,max_tokens=maximum).cpu().tolist()
        memory,context,hidden,padding=model.encode(x,n,a)
        probability=model.decode(p[:,:1],x,memory,context,hidden,padding)[0][:,0].cpu()
        pa=probability[:,66];pb=probability[:,67];conditional=pa/(pa+pb).clamp_min(1e-9)
        for r,tokens,prob,mass in zip(part,decoded,conditional.tolist(),(pa+pb).tolist()):
            output.append({**r,'generated':codec.decode(tokens),'generated_tokens':tokens,
                'conditional_A_probability':prob,'A_B_probability_mass':mass,
                'yes_probability':prob if r.get('yes_is_A',True) else 1-prob})
    return output


def evaluate(args):
    torch.set_num_threads(4);device=torch.device(args.device)
    args.output.mkdir(parents=True,exist_ok=True)
    freeze=dict(checkpoint_sha256=digest(args.checkpoint),pairs_sha256=digest(args.pairs),states_sha256=digest(args.states),selection='Weights frozen before reading held-out scores')
    frozen=args.output/'frozen-test-manifest.json'
    if frozen.exists():assert json.loads(frozen.read_text())==freeze,'Test weights/data changed after evaluation'
    else:frozen.write_text(json.dumps(freeze,indent=2))
    checkpoint=json.loads(args.checkpoint.read_text());payload=checkpoint['sequence_decoder']
    codec=Tokenizer(checkpoint['dialogue']['tokens'],checkpoint['dialogue']['merges']);backend=codec.fast_backend()
    model=SequenceDecoder(**payload['settings']).to(device)
    model.load_state_dict({k:torch.tensor(v,device=device) for k,v in payload['weights'].items()});model.eval()
    ordinary=[{**r,'activity':[0.0]*84} for r in json.loads(args.pairs.read_text()) if r['split']=='test']
    monitor=[{**r,'activity':r['features'],'input':backend.encode(r['question']).ids,'target':backend.encode(r['answer']).ids+[0]} for r in json.loads(args.states.read_text())['rows'] if r['split']=='test']
    language=training.evaluate(model,ordinary,device,batch_size=32 if model.settings['hidden']<=256 else 2)
    answers=predict(model,ordinary,codec,device,64)
    exact=lambda text:' '.join(text.casefold().split())
    language['normalized_exact_match']=np.mean([exact(r['generated'])==exact(r['answer']) for r in answers])
    language['questions']=len(answers)
    reports=predict(model,monitor,codec,device,4)
    primary=[r for r in reports if r['direction']=='observed_activity' and abs(r['strength']-.3)<1e-6]
    condition=[]
    for direction in ['observed_activity','matched_random']:
        for strength in [.1,.3,1,.2,.6]:
            subset=[r for r in reports if r['direction']==direction and abs(r['strength']-strength)<1e-6]
            condition.append(dict(direction=direction,strength=strength,**monitor_metrics(subset)))
    ablated=[{**r,'activity':model.state_mean.cpu().tolist()} for r in primary]
    ablated_reports=predict(model,ablated,codec,device,4)
    by_pair={(r['id'],r['lifecycle'],r['direction'],round(r['strength'],5),r['yes_is_A'],r['injected']):r for r in primary}
    swapped=[{**r,'activity':by_pair[(r['id'],r['lifecycle'],r['direction'],round(r['strength'],5),r['yes_is_A'],not r['injected'])]['features']} for r in primary]
    swapped_reports=predict(model,swapped,codec,device,4)
    baseline=None
    if args.baseline_cache:
        old=json.loads(args.baseline.read_text());index=json.loads((args.baseline_cache/'index.json').read_text())
        x=np.memmap(args.baseline_cache/'features.f32',dtype='<f4',mode='r',shape=(index['rows'],index['features']))
        y=np.memmap(args.baseline_cache/'targets.u16',dtype='<u2',mode='r')
        w=torch.tensor(old['dialogue']['readout']['weights']);b=torch.tensor(old['dialogue']['readout']['bias']);loss=correct=count=0
        for row in index['examples']:
            if row['record']['split']!='test':continue
            at=slice(row['offset'],row['offset']+row['length']);features=torch.from_numpy(x[at].copy());target=torch.tensor(y[at].astype(np.int64));logits=features@w.T+b
            loss+=torch.nn.functional.cross_entropy(logits,target,reduction='sum').item();correct+=(logits.argmax(-1)==target).sum().item();count+=len(target)
        baseline=dict(nll=loss/count,token_accuracy=correct/count,tokens=count)
    result=dict(freeze=freeze,language=language,old_circuit_token_readout=baseline,
        monitor_primary=monitor_metrics(primary),monitor_all=monitor_metrics(reports),
        monitor_by_lifecycle={life:monitor_metrics([r for r in reports if r['lifecycle']==life]) for life in ['post_question','background','rest','inactive']},
        monitor_conditions=condition,state_ablation=monitor_metrics(ablated_reports),paired_state_swap=monitor_metrics(swapped_reports),
        limits=['Fresh groups/templates/direction sources excluded from this adaptation; historical checkpoint exposure unknown.',
            'All answer letters are generated by the learned decoder without a reporting formatter.',
            'Controlled perturbation detection is not evidence of subjective consciousness or semantic introspection.',
            'Exact match is conservative for free-form answers; no human semantic correctness grading was performed.'])
    for name,value in [('results.json',result),('language-predictions.json',answers),('monitor-predictions.json',reports),('ablation-predictions.json',ablated_reports),('swap-predictions.json',swapped_reports)]:
        (args.output/name).write_text(json.dumps(value,indent=2))
    print(json.dumps({k:v for k,v in result.items() if k not in ['monitor_conditions','limits']},indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ['checkpoint','pairs','states','output']:parser.add_argument('--'+name,type=Path,required=True)
    parser.add_argument('--device',default='mps');parser.add_argument('--baseline',type=Path);parser.add_argument('--baseline-cache',type=Path)
    evaluate(parser.parse_args())
