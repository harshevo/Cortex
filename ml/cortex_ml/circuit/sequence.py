"""Learned attentive sequence decoder conditioned on observable circuit activity."""

import torch
from torch import nn


class SequenceDecoder(nn.Module):
    def __init__(self, vocabulary, hidden=128, embedding=96, state_features=84):
        super().__init__()
        if not 257<=vocabulary<=8192 or not 16<=hidden<=1024 or not 16<=embedding<=512 or state_features!=84:
            raise ValueError('Unsupported sequence decoder dimensions')
        self.settings = dict(vocabulary=vocabulary, hidden=hidden, embedding=embedding,
                             state_features=state_features)
        self.embedding = nn.Embedding(vocabulary + 1, embedding)
        self.encoder_forward = nn.GRU(embedding, hidden, batch_first=True)
        self.encoder_reverse = nn.GRU(embedding, hidden, batch_first=True)
        self.state_input = nn.Linear(state_features, hidden)
        self.initial = nn.Linear(hidden * 3, hidden)
        self.decoder = nn.GRU(embedding + hidden * 3, hidden, batch_first=True)
        self.keys = nn.Linear(hidden * 2, hidden)
        self.query = nn.Linear(hidden, hidden)
        self.output_hidden = nn.Linear(hidden * 3 + embedding, hidden)
        self.output = nn.Linear(hidden, vocabulary)
        self.copy_gate = nn.Linear(hidden * 3 + embedding, 1)
        self.register_buffer("state_mean", torch.zeros(state_features))
        self.register_buffer("state_scale", torch.ones(state_features))

    def encode(self, source, lengths, activity):
        positions = torch.arange(source.shape[1], device=source.device)[None, :]
        reverse_index = (lengths[:, None] - 1 - positions).clamp(min=0)
        embedded = self.embedding(source)
        forward, _ = self.encoder_forward(embedded)
        reverse, _ = self.encoder_reverse(self.embedding(source.gather(1, reverse_index)))
        backward = reverse.gather(1, reverse_index[:, :, None].expand_as(reverse))
        memory = torch.cat((forward, backward), -1)
        at = torch.arange(len(source), device=source.device)
        summary = torch.cat((forward[at, lengths - 1], reverse[at, lengths - 1]), -1)
        normalized = ((activity - self.state_mean) / self.state_scale).clamp(-8, 8)
        state = torch.tanh(self.state_input(normalized))
        context = torch.cat((summary, state), -1)
        initial = torch.tanh(self.initial(context)).unsqueeze(0)
        return memory, context, initial, positions >= lengths[:, None]

    def decode(self, previous, source, memory, context, hidden, padding):
        embedded = self.embedding(previous)
        value, hidden = self.decoder(torch.cat((embedded,
            context[:, None].expand(-1, previous.shape[1], -1)), -1), hidden)
        attention = torch.bmm(self.query(value), self.keys(memory).transpose(1, 2))
        attention = (attention / self.settings["hidden"] ** 0.5).masked_fill(
            padding[:, None], -1e4).softmax(-1)
        attended = torch.bmm(attention, memory)
        combined = torch.cat((value, attended, embedded), -1)
        generated = self.output(torch.tanh(self.output_hidden(combined))).softmax(-1)
        gate = self.copy_gate(combined).sigmoid()
        copied = torch.zeros_like(generated).scatter_add(
            2, source[:, None].expand(-1, previous.shape[1], -1), attention)
        return (gate * generated + (1 - gate) * copied).clamp_min(1e-9), hidden

    def forward(self, source, lengths, previous, activity):
        memory, context, hidden, padding = self.encode(source, lengths, activity)
        return self.decode(previous, source, memory, context, hidden, padding)[0]

    @torch.no_grad()
    def generate(self, source, lengths, activity, max_tokens=64):
        self.eval()
        memory, context, hidden, padding = self.encode(source, lengths, activity)
        previous = torch.full((len(source), 1), self.settings["vocabulary"],
                              device=source.device, dtype=torch.long)
        output, active = [], torch.ones(len(source), dtype=torch.bool, device=source.device)
        for _ in range(max_tokens):
            probabilities, hidden = self.decode(previous, source, memory, context, hidden, padding)
            token = probabilities[:, -1].argmax(-1)
            token = torch.where(active, token, 0)
            output.append(token)
            active &= token != 0
            previous = token[:, None]
            if not active.any():
                break
        return torch.stack(output, 1)

    def portable(self, training):
        return dict(version=1, settings=self.settings, training=training,
                    weights={name: value.detach().cpu().tolist()
                             for name, value in self.state_dict().items()})


def batch(records, vocabulary, device):
    length = max(len(row["input"]) for row in records)
    answers = max(len(row["target"]) for row in records)
    source = torch.zeros(len(records), length, dtype=torch.long)
    previous = torch.zeros(len(records), answers, dtype=torch.long)
    targets = torch.full((len(records), answers), -100, dtype=torch.long)
    for index, row in enumerate(records):
        source[index, :len(row["input"])] = torch.tensor(row["input"])
        target = row["target"]
        previous[index, :len(target)] = torch.tensor([vocabulary] + target[:-1])
        targets[index, :len(target)] = torch.tensor(target)
    return tuple(value.to(device) for value in (
        source, torch.tensor([len(row["input"]) for row in records]), previous,
        torch.tensor([row["activity"] for row in records]), targets))


def activity_features(core, state):
    populations=[row['population'] for row in core.template['network']['neurons']]
    values=[];slots=core.branch_slots
    for name in ['Association','Episodic','Context','Action','Value','Inhibitory']:
        indices=[i for i,population in enumerate(populations) if population==name]
        if not indices:
            values.extend([state.voltage.new_zeros(len(state.voltage))]*14);continue
        voltage=state.voltage[:,indices];fast=state.fast_trace[:,indices]/10;trace=state.trace[:,indices]/10
        mask=core.branch_mask[indices];count=mask.sum().clamp_min(1)
        branch=(state.branch_voltage[:,indices]*mask).sum((1,2))/count
        plateau=((state.plateau[:,indices]>0)*mask).sum((1,2))/count
        values.extend([voltage.mean(1),voltage.std(1,unbiased=False),fast.mean(1),fast.std(1,unbiased=False),
            trace.mean(1),trace.std(1,unbiased=False),state.currents[:,indices,slots].mean(1),
            state.currents[:,indices,slots*2+1].mean(1),(state.refractory[:,indices]>0).float().mean(1),
            branch,plateau,state.adaptation[:,indices].mean(1),voltage.min(1).values,voltage.max(1).values])
    return torch.stack(values,1)


class SequenceSession:
    """Matching stateful Python inference for notebook testing of native checkpoints."""
    def __init__(self,checkpoint,device='cpu'):
        from .model import Circuit
        from .tokenizer import Tokenizer
        self.core=Circuit(checkpoint).to(device).eval()
        for parameter in self.core.parameters():parameter.requires_grad_(False)
        payload=checkpoint['sequence_decoder']
        self.decoder=SequenceDecoder(**payload['settings']).to(device).eval()
        self.decoder.load_state_dict({key:torch.tensor(value,device=device) for key,value in payload['weights'].items()})
        self.codec=Tokenizer(checkpoint['dialogue']['tokens'],checkpoint['dialogue']['merges'])
        self.ticks=checkpoint['dialogue']['ticks_per_symbol'];self.reset()

    def reset(self):self.state=self.core.initial(1)

    @torch.no_grad()
    def feed(self,tokens):
        from .data import token_spikes
        spikes=token_spikes(torch.tensor([tokens]),self.core.inputs,self.ticks,len(self.codec.tokens)).to(self.core.magnitudes.device)
        _,self.state=self.core(spikes,state=self.state,head='features');self.state=self.state.detach()

    @torch.no_grad()
    def ask(self,question,max_tokens=64):
        tokens=self.codec.encode(question)
        if not tokens or len(tokens)>512 or len(question.encode())>8192:raise ValueError('Question exceeds decoder limits')
        device=self.core.magnitudes.device;source=torch.tensor([tokens],device=device)
        generated=self.decoder.generate(source,torch.tensor([len(tokens)],device=device),activity_features(self.core,self.state),max_tokens).cpu().tolist()[0]
        if 0 in generated:generated=generated[:generated.index(0)]
        self.feed([len(self.codec.tokens)]+tokens+[len(self.codec.tokens)+1]+generated+[0])
        return dict(text=self.codec.decode(generated),tokens=generated,method='learned_attentive_circuit_decoder')

    @torch.no_grad()
    def idle(self,ticks=64):
        if not 1<=ticks<=10000:raise ValueError('Idle ticks must be 1..10000')
        _,self.state=self.core(torch.zeros(1,ticks,self.core.inputs,device=self.core.magnitudes.device),state=self.state,head='features')
