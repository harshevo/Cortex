import copy
import importlib.util
import json
import subprocess
import socket
import time
import tempfile
from pathlib import Path
from types import SimpleNamespace

import pytest
import torch

from cortex_ml.circuit.data import token_spikes
from cortex_ml.circuit.model import Circuit
from cortex_ml.circuit.sequence import SequenceDecoder,SequenceSession,activity_features
from cortex_ml.circuit.tokenizer import Tokenizer

ROOT=Path(__file__).resolve().parents[2]
BINARY=ROOT/'target/release/cortex-brain-circuit'


@pytest.fixture
def learned_checkpoint(tmp_path):
    if not BINARY.exists():pytest.skip('Build native circuit first')
    path=tmp_path/'seed.json'
    subprocess.run([str(BINARY),'init',str(path),'small'],check=True)
    checkpoint=json.loads(path.read_text());codec=Tokenizer();size=len(checkpoint['network']['neurons'])
    checkpoint['dialogue']=dict(tokens=codec.tokens,merges=[],ticks_per_symbol=2,fast_readout=True,
        readout=dict(weights=[[0.0]*size for _ in codec.tokens],bias=[0.0]*len(codec.tokens),updates=0))
    torch.manual_seed(17);decoder=SequenceDecoder(len(codec.tokens),hidden=16,embedding=16)
    checkpoint['sequence_decoder']=decoder.portable({})
    path.write_text(json.dumps(checkpoint))
    return checkpoint,decoder,path


def test_sequence_decoder_rust_probabilities_match(learned_checkpoint,tmp_path):
    checkpoint,decoder,path=learned_checkpoint;codec=Tokenizer();text='Unseen question Ω'
    activity=torch.randn(1,84);features=tmp_path/'features.json';features.write_text(json.dumps(activity[0].tolist()))
    tokens=torch.tensor([codec.encode(text)]);lengths=torch.tensor([tokens.shape[1]])
    with torch.no_grad():
        memory,context,hidden,padding=decoder.encode(tokens,lengths,activity)
        previous=torch.tensor([[len(codec.tokens)]])
        expected=decoder.decode(previous,tokens,memory,context,hidden,padding)[0][0,0]
        generated=decoder.generate(tokens,lengths,activity,64)[0].tolist()
    if 0 in generated:generated=generated[:generated.index(0)]
    actual=json.loads(subprocess.check_output([str(BINARY),'sequence-probe',str(path),str(features),text]))
    assert torch.allclose(expected,torch.tensor(actual['first_token_probabilities']),atol=1e-5)
    assert actual['tokens']==generated


def test_activity_features_and_continuous_notebook_chat_match_rust(learned_checkpoint):
    checkpoint,_,path=learned_checkpoint;codec=Tokenizer();core=Circuit(checkpoint)
    text='Check an unseen internal state'
    with torch.no_grad():
        _,state=core(token_spikes(torch.tensor([[257]+codec.encode(text)+[258]]),512,2,257),head='features')
    actual=json.loads(subprocess.check_output([str(BINARY),'activity-probe',str(path),text]))
    assert torch.allclose(activity_features(core,state)[0],torch.tensor(actual),atol=2e-4)
    session=SequenceSession(checkpoint)
    expected=[session.ask(q)['tokens'] for q in ['hello','state?']]
    from cortex_ml.circuit.portable import load_session
    portable=load_session(path,device='cpu')
    assert [portable.ask(q)['tokens'] for q in ['hello','state?']]==expected
    raw=subprocess.run([str(BINARY),'stream',str(path)],input='hello\nstate?\n/quit\n',text=True,capture_output=True,check=True)
    assert [json.loads(line)['tokens'] for line in raw.stdout.splitlines()]==expected


def test_instruction_answers_depend_on_weights_without_executor(learned_checkpoint):
    checkpoint,decoder,path=learned_checkpoint
    with torch.no_grad():
        decoder.output.weight.zero_();decoder.output.bias.fill_(-30);decoder.output.bias[0]=30
        decoder.copy_gate.weight.zero_();decoder.copy_gate.bias.fill_(50)
    checkpoint['sequence_decoder']=decoder.portable({});path.write_text(json.dumps(checkpoint))
    result=json.loads(subprocess.check_output([str(BINARY),'generate',str(path),'Reply in JSON: calculate: (17 + 32) * 2']))
    assert result['text']=='' and result['tokens']==[]
    assert result['method']=='learned_attentive_circuit_decoder'


def test_monitor_command_uses_active_learned_head(learned_checkpoint):
    checkpoint,decoder,path=learned_checkpoint
    checkpoint['experience']=[dict(question=q,label=None,reference='',source='unit',domain='unit') for q in ['hello','Describe the changing internal activity over a longer processing sequence.']]
    with torch.no_grad():
        decoder.output.weight.zero_();decoder.output.bias.fill_(-30);decoder.output.bias[66]=30
        decoder.copy_gate.weight.zero_();decoder.copy_gate.bias.fill_(50)
    checkpoint['sequence_decoder']=decoder.portable({});path.write_text(json.dumps(checkpoint))
    result=json.loads(subprocess.check_output([str(BINARY),'monitor',str(path)]))
    assert result['summary']['unconstrained_A_B_compliance']==1.0
    assert all(row['decision']['decoder_method']=='learned_attentive_circuit_decoder' for row in result['trials'])


def test_full_512_symbol_instruction_fits_native_and_notebook(learned_checkpoint):
    _,_,path=learned_checkpoint
    text='a'*512
    result=json.loads(subprocess.check_output([str(BINARY),'generate',str(path),text]))
    assert result['method']=='learned_attentive_circuit_decoder'
    from cortex_ml.circuit.portable import load_session
    assert load_session(path,device='cpu').ask(text,max_tokens=1)['method']==result['method']


def test_teacher_forcing_cannot_see_future_answer_symbols():
    torch.manual_seed(19);model=SequenceDecoder(257,hidden=16,embedding=16)
    source=torch.tensor([[14,34,57,10]]);lengths=torch.tensor([4]);previous=torch.tensor([[257,10,20]])
    other=previous.clone();other[:,1:]=200
    first=model(source,lengths,previous,torch.zeros(1,84))[:,0]
    second=model(source,lengths,other,torch.zeros(1,84))[:,0]
    assert torch.equal(first,second)
    loss=-first[:,33].log().mean();loss.backward()
    for name,parameter in model.named_parameters():
        assert parameter.grad is not None and torch.isfinite(parameter.grad).all(),name


def test_portable_30m_preset_has_active_trainable_parameters():
    with torch.device('meta'):model=SequenceDecoder(512,hidden=960,embedding=512)
    assert sum(p.numel() for p in model.parameters() if p.requires_grad)==30659201


def test_native_replay_gradient_matches_pytorch_and_persists(learned_checkpoint):
    checkpoint,decoder,path=learned_checkpoint;codec=Tokenizer();text='hello'
    target=codec.encode('h')+[0]
    row=dict(question=text,activity=[0.0]*84,target=target)
    checkpoint['sequence_decoder']['training'].update(replay_examples=[row],retention_examples=[row])
    path.write_text(json.dumps(checkpoint))
    source=torch.tensor([codec.encode(text)]);lengths=torch.tensor([source.shape[1]])
    previous=torch.tensor([[257]+target[:-1]])
    decoder.zero_grad();p=decoder(source,lengths,previous,torch.zeros(1,84))
    torch.nn.functional.nll_loss(p.log().flatten(0,1),torch.tensor(target)).backward()
    expected=decoder.output.bias.detach()-0.001*decoder.output.bias.grad
    subprocess.run([str(BINARY),'consolidate',str(path)],check=True,capture_output=True)
    saved=json.loads(path.read_text())
    assert saved['adaptation']['sequence_replay_accepted']==1
    assert torch.allclose(torch.tensor(saved['sequence_decoder']['weights']['output.bias']),expected,atol=1e-6)


def test_corrupt_replay_never_replaces_checkpoint(learned_checkpoint):
    checkpoint,_,path=learned_checkpoint
    checkpoint['sequence_decoder']['training'].update(replay_examples=[dict(question='hello',activity=[0.0]*83,target=[105,0])],retention_examples=[dict(question='hello',activity=[0.0]*84,target=[105,0])])
    path.write_text(json.dumps(checkpoint));before=path.read_bytes()
    result=subprocess.run([str(BINARY),'consolidate',str(path)],capture_output=True)
    assert result.returncode!=0
    assert path.read_bytes()==before


def test_native_negative_feedback_updates_learned_weights(learned_checkpoint,tmp_path):
    checkpoint,_,path=learned_checkpoint
    row=dict(question='hello',activity=[0.0]*84,target=[105,0])
    checkpoint['sequence_decoder']['training'].update(replay_examples=[row],retention_examples=[row])
    path.write_text(json.dumps(checkpoint));socket_directory=tempfile.TemporaryDirectory(prefix='cx-',dir='/tmp');endpoint=Path(socket_directory.name)/'brain.sock'
    process=subprocess.Popen([str(BINARY),'serve',str(path),str(endpoint)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    def call(command,**fields):
        with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as client:
            client.settimeout(20);client.connect(str(endpoint));client.sendall(json.dumps(dict(command=command,**fields)).encode()+b'\n')
            result=json.loads(client.makefile('rb').readline())
        assert 'error' not in result,result
        return result['result']
    try:
        for _ in range(100):
            if endpoint.exists():break
            time.sleep(.02)
        answer=call('dialogue',text='hello');assert answer['feedback_supported']
        call('feedback',reward=-1.0)
        saved=json.loads(path.read_text())
        assert saved['adaptation']['sequence_feedback_updates']==1
        assert saved['sequence_decoder']['weights']['output.bias']!=checkpoint['sequence_decoder']['weights']['output.bias']
        call('dialogue',text='hello')
        before=call('state')['updates'];backup=path.with_suffix('.backup');path.rename(backup);path.mkdir()
        with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as client:
            client.settimeout(20);client.connect(str(endpoint));client.sendall(b'{"command":"feedback","reward":-1.0}\n')
            failure=json.loads(client.makefile('rb').readline())
        assert 'error' in failure
        assert 'directory' in failure['error'].lower()
        assert call('state')['updates']==before
        path.rmdir();backup.rename(path)
    finally:
        process.terminate();process.wait(timeout=5)
        socket_directory.cleanup()


def test_test_data_cannot_change_training_or_checkpoint_selection(learned_checkpoint,tmp_path):
    checkpoint,_,path=learned_checkpoint
    spec=importlib.util.spec_from_file_location('sequence_training',ROOT/'scripts/train-learned-instructions.py')
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    codec=Tokenizer();pairs=[];states=[]
    for split in ['train','dev','test']:
        pairs.append(dict(id=split,group=split,split=split,question='hello',answer='hi',input=codec.encode('hello'),target=codec.encode('hi')+[0]))
        for injected in [False,True]:states.append(dict(id=split,group=split,split=split,question='Is it changed? Reply A for yes or B for no.',answer='A' if injected else 'B',injected=injected,features=[float(injected)]*84))
    outputs=[]
    for iteration in range(2):
        corpus=tmp_path/f'pairs-{iteration}.json';activity=tmp_path/f'states-{iteration}.json';output=tmp_path/f'run-{iteration}'
        modified=copy.deepcopy(states)
        if iteration:
            for row in modified:
                if row['split']=='test':row['features']=[1000.0]*84;row['answer']='B' if row['answer']=='A' else 'A'
        corpus.write_text(json.dumps(pairs));activity.write_text(json.dumps(dict(core_signature=module.core_signature(checkpoint),rows=modified)))
        module.train(SimpleNamespace(checkpoint=path,pairs=corpus,states=activity,output=output,steps=1,batch_size=4,device='cpu',lr=.001,resume=None,hidden=16,embedding=16))
        outputs.append(json.loads((output/'checkpoint.json').read_text())['sequence_decoder'])
    assert outputs[0]==outputs[1]
