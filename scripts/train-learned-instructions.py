#!/usr/bin/env python3
"""Learn answers and hidden-state reports without task-specific inference handlers."""
import argparse
import copy
import hashlib
import json
import random
import sys
import struct
import time
from pathlib import Path

import torch
from torch import nn

sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'ml'))
from cortex_ml.circuit.sequence import SequenceDecoder, batch
from cortex_ml.circuit.tokenizer import Tokenizer


def write_json(path,value):
    temporary=path.with_suffix(path.suffix+'.tmp')
    temporary.write_text(json.dumps(value,separators=(',',':'),allow_nan=False));temporary.replace(path)


def write_torch(path,value):
    temporary=path.with_suffix(path.suffix+'.tmp')
    torch.save(value,temporary);temporary.replace(path)


def core_signature(checkpoint):
    value=1469598103934665603
    weights=[edge['magnitude'] for edge in checkpoint['network']['synapses']]
    for cell in checkpoint['network']['neurons']:
        weights.append(cell.get('threshold_scale',1.0));weights.extend(branch.get('gain',1.0) for branch in cell['branches'])
    for weight in weights:value=((value^struct.unpack('<I',struct.pack('<f',weight))[0])*1099511628211)&((1<<64)-1)
    return value


def replay_metadata(model,language,monitor,device):
    if model.settings['hidden']>256:return {'online_replay_scope':'Use GPU/offline replay for this larger decoder'}
    ordered=lambda rows:sorted(rows,key=lambda row:(len(row['target']),row['id']))
    train=ordered(language['train'])[:4];retention=ordered(language['dev'])[:4]
    for lifecycle in ['post_question','background','rest','inactive']:
        for injected in [False,True]:
            subset=[row for row in monitor['dev'] if row.get('lifecycle')==lifecycle and row['injected']==injected]
            if subset:retention.append(subset[0])
    def packed(row):return {key:row[key] for key in ['question','activity','target']}
    reference=[]
    with torch.no_grad():
        for row in retention:
            x,n,p,a,y=batch([row],model.settings['vocabulary'],device)
            reference.append(nn.functional.nll_loss(model(x,n,p,a).log().flatten(0,1),y.flatten()).item())
    return dict(replay_examples=[packed(row) for row in train],retention_examples=[packed(row) for row in retention],
        retention_reference_losses=reference,online_replay_scope='Only output biases update in native replay/feedback, bounded by a small fixed retention set. This is a proxy, not a global quality guarantee.')


@torch.no_grad()
def evaluate(model,rows,device,batch_size=32):
    model.eval();loss=correct=count=0;first=first_correct=0
    for start in range(0,len(rows),batch_size):
        source,lengths,previous,activity,target=batch(rows[start:start+batch_size],model.settings['vocabulary'],device)
        probability=model(source,lengths,previous,activity)
        loss+=nn.functional.nll_loss(probability.log().flatten(0,1),target.flatten(),ignore_index=-100,reduction='sum').item()
        valid=target!=-100;correct+=((probability.argmax(-1)==target)&valid).sum().item();count+=valid.sum().item()
        first_correct+=(probability[:,0].argmax(-1)==target[:,0]).sum().item();first+=len(source)
    return dict(nll=loss/count,token_accuracy=correct/count,first_token_accuracy=first_correct/first,tokens=count)


def train(args):
    if not 1<=args.steps<=1000000 or not 1<=args.batch_size<=128 or not 0<args.lr<=0.01:raise ValueError('Invalid training budget or learning rate')
    torch.set_num_threads(4);torch.manual_seed(1731);rng=random.Random(1731)
    device=torch.device(args.device)
    base=json.loads(args.checkpoint.read_text());codec=Tokenizer(base['dialogue']['tokens'],base['dialogue']['merges']);backend=codec.fast_backend()
    original=json.loads(args.pairs.read_text());monitoring=json.loads(args.states.read_text())
    split_groups={}
    for row in original+monitoring['rows']:
        if row['split'] not in ['train','dev','test']:raise ValueError('Invalid split')
        if row['group'] in split_groups and split_groups[row['group']]!=row['split']:raise ValueError('Conversation group crosses splits')
        split_groups[row['group']]=row['split']
    rows=monitoring['rows'];features=torch.tensor([row['features'] for row in rows if row['split']=='train'])
    if core_signature(base)!=monitoring['core_signature']:raise ValueError('Monitoring data belongs to a different neural core')
    settings=base.get('sequence_decoder',{}).get('settings',{})
    hidden=args.hidden or settings.get('hidden',128);embedding=args.embedding or settings.get('embedding',96)
    model=SequenceDecoder(len(codec.tokens),hidden=hidden,embedding=embedding).to(device)
    if base.get('sequence_decoder'):
        model.load_state_dict({name:torch.tensor(value,device=device) for name,value in base['sequence_decoder']['weights'].items()})
    model.state_mean.copy_(features.mean(0).to(device));scale=features.std(0);scale=torch.where(scale>1e-6,scale,torch.ones_like(scale));model.state_scale.copy_(scale.to(device))
    language={s:[] for s in ['train','dev','test']};monitor={s:[] for s in language}
    normal=[row['features'] for row in rows if row['split']=='train' and not row['injected']]
    for row in original:
        language[row['split']].append({**row,'activity':[0.0]*84})
    for row in rows:
        monitor[row['split']].append({**row,'activity':row['features'],
            'input':backend.encode(row['question']).ids,'target':backend.encode(row['answer']).ids+[0]})
    optimizer=torch.optim.AdamW(model.parameters(),lr=args.lr,weight_decay=0.001)
    args.output.mkdir(parents=True,exist_ok=True)
    if (args.output/'checkpoint.json').exists() and not args.resume:raise ValueError('Use a new output directory or explicitly resume')
    history=[];start_step=0;hard=[]
    saved=None
    if args.resume:
        saved=torch.load(args.resume,map_location='cpu',weights_only=False)
        assert saved['pairs_sha256']==hashlib.sha256(args.pairs.read_bytes()).hexdigest()
        assert saved['states_sha256']==hashlib.sha256(args.states.read_bytes()).hexdigest()
        model.load_state_dict(saved['model']);optimizer.load_state_dict(saved['optimizer'])
        rng.setstate(saved['rng']);torch.set_rng_state(saved['torch_rng']);start_step=saved['step'];history=saved['history']
    before={k:evaluate(model,data['dev'],device,batch_size=args.batch_size) for k,data in [('language',language),('monitor',monitor)]}
    best_score=before['language']['nll']+before['monitor']['nll'];best_metrics=before;best_step=start_step
    best=copy.deepcopy({k:v.detach().cpu() for k,v in model.state_dict().items()})
    if saved and 'best_model' in saved:
        best=saved['best_model'];best_score=saved['best_score'];best_metrics=saved['best_metrics'];best_step=saved['best_step'];before=saved['before'];hard=saved['hard']
    initial={k:v.clone() for k,v in best.items()}
    start=time.monotonic()
    for step in range(start_step,start_step+args.steps):
        model.train();selected=[]
        for i in range(args.batch_size):
            if i%2:
                item=copy.copy(rng.choice(hard if hard and rng.random()<0.5 else language['train']))
                item['activity']=rng.choice(normal) if rng.random()<0.75 else [0.0]*84
            else:item=rng.choice(monitor['train'])
            selected.append(item)
        source,lengths,previous,activity,target=batch(selected,len(codec.tokens),device)
        optimizer.zero_grad(set_to_none=True)
        probability=model(source,lengths,previous,activity)
        token_loss=nn.functional.nll_loss(probability.log().flatten(0,1),target.flatten(),ignore_index=-100,reduction='none').view_as(target)
        loss=(token_loss.sum(1)/(target!=-100).sum(1)).mean()
        loss.backward();nn.utils.clip_grad_norm_(model.parameters(),1,error_if_nonfinite=True);optimizer.step()
        if step==start_step or (step+1)%100==0:
            current={k:evaluate(model,data['dev'],device,batch_size=args.batch_size) for k,data in [('language',language),('monitor',monitor)]}
            score=current['language']['nll']+current['monitor']['nll']
            retained=all(current[k]['nll']<=best_metrics[k]['nll']*1.02 for k in current)
            promoted=score<best_score and retained
            if promoted:
                best_score,best_metrics,best_step=score,current,step+1
                best=copy.deepcopy({k:v.detach().cpu() for k,v in model.state_dict().items()})
            event=dict(step=step+1,training_loss=loss.item(),development=current,promoted=promoted,
                       replay_examples=len(hard),elapsed_seconds=time.monotonic()-start)
            history.append(event);print(json.dumps(event),flush=True)
            write_json(args.output/'progress.json',dict(before=before,best_step=best_step,best=best_metrics,history=history))
        if (step+1)%400==0:
            # Rank only training examples; test records never drive replay or promotion.
            candidates=rng.sample(language['train'],min(256,len(language['train'])))
            ranked=[]
            for j in range(0,len(candidates),args.batch_size):
                part=candidates[j:j+args.batch_size];x,n,p,a,y=batch(part,len(codec.tokens),device)
                with torch.no_grad():
                    losses=nn.functional.nll_loss(model(x,n,p,a).log().flatten(0,1),y.flatten(),ignore_index=-100,reduction='none').view_as(y)
                    losses=(losses.sum(1)/(y!=-100).sum(1)).cpu().tolist()
                ranked.extend(zip(losses,part))
            hard=[row for _,row in sorted(ranked,key=lambda pair:pair[0],reverse=True)[:64]]
            saved_path=args.output/f'training-step-{step+1:05}.pt'
            write_torch(saved_path,dict(model={k:v.detach().cpu() for k,v in model.state_dict().items()},optimizer=optimizer.state_dict(),rng=rng.getstate(),torch_rng=torch.get_rng_state(),step=step+1,history=history,pairs_sha256=hashlib.sha256(args.pairs.read_bytes()).hexdigest(),states_sha256=hashlib.sha256(args.states.read_bytes()).hexdigest(),best_model=best,best_score=best_score,best_metrics=best_metrics,best_step=best_step,before=before,hard=hard,settings=model.settings,core_signature=monitoring['core_signature']))
            callback=getattr(args,'on_checkpoint',None)
            if callback:callback(saved_path)
    model.load_state_dict(best)
    metadata=dict(method='learned_attentive_recurrent_decoder',selected_step=best_step,
        core_signature=monitoring['core_signature'],source_answers='original user corpus only',
        monitor_labels='instrumented interventions/shams',selection='development losses with 2 percent retention tolerance; no test selection',
        self_improvement='training-error replay with development retention gates; generated answers never become trusted labels')
    metadata.update(replay_metadata(model,language,monitor,device))
    base['sequence_decoder']=model.portable(metadata)
    write_json(args.output/'checkpoint.json',base)
    write_torch(args.output/'model.pt',dict(settings=model.settings,model=best))
    write_json(args.output/'training-results.json',dict(before=before,best=best_metrics,best_step=best_step,
        parameters=sum(p.numel() for p in model.parameters()),history=history,training_counts={k:len(v['train']) for k,v in [('language',language),('monitor',monitor)]},
        parameter_max_change={k:float((v-initial[k]).abs().max()) for k,v in best.items() if k not in ['state_mean','state_scale']},metadata=metadata))
    print(json.dumps(dict(saved=str(args.output/'checkpoint.json'),best_step=best_step,best=best_metrics)),flush=True)


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ['checkpoint','pairs','states','output']:parser.add_argument('--'+name,type=Path,required=True)
    parser.add_argument('--resume',type=Path);parser.add_argument('--steps',type=int,default=2400);parser.add_argument('--batch-size',type=int,default=24);parser.add_argument('--device',default='mps');parser.add_argument('--lr',type=float,default=0.002)
    parser.add_argument('--hidden',type=int);parser.add_argument('--embedding',type=int)
    train(parser.parse_args())
