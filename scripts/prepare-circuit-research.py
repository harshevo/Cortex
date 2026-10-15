#!/usr/bin/env python3
"""Local-only curriculum and untouched upstream evaluation for the new circuit."""
import argparse
import ast
import collections
import gzip
import hashlib
import json
import pathlib
import random
import re
from decimal import Decimal
from fractions import Fraction

NUMBER = r"(?<![\w.])\d+(?:,\d{3})*(?:\.\d+)?"


def identity(text):
    return hashlib.sha256(text.strip().lower().encode()).hexdigest()


def program(question, answer):
    numbers = re.findall(NUMBER, question)
    if not 2 <= len(numbers) <= 5:
        return None
    known = {}
    for i, value in enumerate(numbers):
        known.setdefault(Decimal(value.replace(',', '')), chr(97+i))
    for expression, value in re.findall(r'<<([^=<>]+)=([^<>]+)>>', answer):
        try:
            def convert(node):
                if isinstance(node, ast.Constant) and isinstance(node.value, (int, float)):
                    val = Decimal(str(node.value))
                    if val in known:
                        return known[val]
                    if val in (Decimal(2), Decimal(100), Decimal('0.5')):
                        return str(val)
                    raise ValueError()
                if isinstance(node, ast.BinOp) and type(node.op) in (ast.Add, ast.Sub, ast.Mult, ast.Div):
                    op = {ast.Add: '+', ast.Sub: '-', ast.Mult: '*', ast.Div: '/'}[type(node.op)]
                    return f'({convert(node.left)}{op}{convert(node.right)})'
                raise ValueError()
            result = convert(ast.parse(expression.replace(',', ''), mode='eval').body)
            known[Decimal(value.replace(',', ''))] = result
        except (ValueError, SyntaxError, ArithmeticError):
            return None
    try:
        final = Decimal(answer.split('####')[-1].strip().replace(',', ''))
        result = known[final]
        if len(result) > 80 or len(result) < 5:
            return None
        return result
    except (KeyError, ArithmeticError):
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=pathlib.Path)
    parser.add_argument('--output', type=pathlib.Path, default=pathlib.Path('target/circuit-research'))
    args = parser.parse_args()
    rng = random.Random(1501)
    train, dev, test, broad = [], [], [], []
    gsm_train, gsm_test = [], []
    source = args.root/'05_math/gsm8k/records.jsonl.gz'
    for line in gzip.open(source, 'rt'):
        row = json.loads(line)
        if not row.get('path', '').endswith(('/train.jsonl', '/test.jsonl')):
            continue
        data = row['data']
        item = dict(question=data['question'], reference=data['answer'].split('####')[-1].strip(),
                    source='gsm8k', source_path=row['path'], id=identity(data['question']), domain='gsm8k')
        item['label'] = program(data['question'], data['answer'])
        (gsm_train if row['path'].endswith('/train.jsonl') else gsm_test).append(item)
    counts = collections.Counter(r['label'] for r in gsm_train if r['label'])
    catalog = [label for label, count in counts.most_common(16) if count >= 8]
    for row in gsm_train:
        if row['label'] in catalog:
            (dev if int(row['id'][:8], 16) % 7 == 0 else train).append(row)
    test += [r for r in gsm_test if r['label'] in catalog]
    broad += gsm_test
    # Generated scaffolds have disjoint language templates and number pairs.
    patterns = {
        '(a+b)': (["{n} has {a} {o} and receives {b} more. How many {o} now?", "Add {a} and {b}.", "What is the sum of {a} and {b}?", "What is {a} + {b}?"], ["{n} collected {a} {o}, then found {b} more. How many altogether?"], ["{n} owns {a} {o}. Someone gives {n} another {b}. What is the total?"]),
        '(a-b)': (["{n} has {a} {o} and gives away {b}. How many remain?", "{a} minus {b}.", "What is {a} minus {b}?", "What is {a} - {b}?"], ["{n} starts with {a} {o} and loses {b}. How many are left?"], ["There were {a} {o}. After {b} were removed, how many are still there?"]),
        '(a*b)': (["There are {a} boxes with {b} {o} each. How many in total?", "Multiply {a} by {b}.", "What is the product of {a} and {b}?", "What is {a} * {b}?"], ["{n} buys {a} packs containing {b} {o} per pack. How many altogether?"], ["{n} fills {a} baskets. Each holds {b} {o}. How many {o} are in all the baskets?"]),
        '(a/b)': (["Divide {a} by {b}.", "What is {a} / {b}?", "{n} shares {a} {o} equally among {b} people. How many per person?"], ["{a} {o} are split equally into {b} groups. How many in each group?"], ["{n} distributes {a} {o} evenly across {b} containers. What is the number per container?"]),
        'force': (["Find force for mass {a} kg and acceleration {b} m/s^2.", "A mass of {a} kg accelerates at {b} m/s^2. Calculate the net force."], ["What net force accelerates {a} kg at {b} m/s^2?"], ["An object has mass {a} kg. Its acceleration is {b} m/s^2. What force acts on it?"]),
        'speed': (["Find speed for distance {a} m and time {b} s.", "Traveling {a} meters in {b} seconds gives what average speed?"], ["What is the speed over {a} m during {b} s?"], ["A runner covers a distance of {a} m in a time of {b} s. Calculate average speed."]),
        'kinetic': (["Find kinetic energy for mass {a} kg and speed {b} m/s.", "Calculate kinetic energy of {a} kg moving at {b} m/s."], ["What kinetic energy does mass {a} kg have at velocity {b} m/s?"], ["An object of mass {a} kg travels at {b} m/s. How much kinetic energy does it carry?"]),
    }
    operations = {
        '(a+b)': ['gets', 'receives', 'finds', 'collects', 'buys', 'adds', 'gains', 'obtains', 'found', 'collected', 'received', 'was given'],
        '(a-b)': ['loses', 'gives away', 'sells', 'removes', 'spends', 'uses', 'discards', 'donates', 'lost', 'gave away', 'removed', 'used'],
    }
    for label, verbs in operations.items():
        for verb in verbs:
            patterns[label][0].extend([f"{{n}} starts with {{a}} {{o}} and {verb} {{b}}. How many are left?", f"There are {{a}} {{o}}. {{n}} {verb} {{b}} {{o}}. Find the new total."])
    patterns['(a*b)'][0].extend(["{n} has {a} baskets, each holding {b} {o}. Find the combined count.", "Each of {a} containers contains {b} {o}. What is the total?", "{n} buys {a} packs containing {b} {o} per pack. How many?", "{n} makes {a} equal groups of {b} {o} each. Find the product."])
    patterns['(a/b)'][0].extend(["Split {a} {o} equally into {b} groups. How many per group?", "Distribute {a} {o} evenly among {b} people. Find each share.", "{a} {o} fill {b} equal containers. How many in each container?", "{n} divides {a} {o} into {b} equal portions. How large is one portion?"])
    patterns['force'][0].extend(["What is the force on mass {a} kg with acceleration {b} m/s^2?", "A {a} kg body accelerates at {b} m/s^2. Determine net force.", "Determine the force required for {a} kilograms at {b} m/s^2.", "A particle of {a} kg has acceleration {b} m/s^2; find force."])
    patterns['speed'][0].extend(["Compute average speed over a distance of {a} m in {b} s.", "What speed corresponds to {a} meters covered during {b} seconds?", "A distance of {a} m is crossed in a time of {b} s. Find speed.", "Determine speed when distance equals {a} m and duration equals {b} s."])
    patterns['kinetic'][0].extend(["What is the kinetic energy of mass {a} kg at speed {b} m/s?", "A particle has mass {a} kg and velocity {b} m/s. Determine kinetic energy.", "How much kinetic energy is in a {a} kg body moving at {b} m/s?", "Mass equals {a} kg and speed equals {b} m/s. Find kinetic energy."])
    catalog = sorted(set(catalog) | set(patterns) | {'greeting', 'identity', 'capabilities', 'unknown'})
    for label, splits in patterns.items():
        for target, templates, offset in [(train, splits[0], 0), (dev, splits[1], 101), (test, splits[2], 301)]:
            for template in templates:
                for i in range(30 if target is train else 12):
                    a, b = rng.randint(10+offset, 90+offset), rng.randint(2+offset, 9+offset)
                    question = template.format(a=a,b=b,n=rng.choice(['Mira','Sam','Ravi','Leah']),o=rng.choice(['apples','coins','books','marbles']))
                    value = {'(a+b)':lambda:a+b,'(a-b)':lambda:a-b,'(a*b)':lambda:a*b,'(a/b)':lambda:Fraction(a,b),'force':lambda:a*b,'speed':lambda:Fraction(a,b),'kinetic':lambda:Fraction(a*b*b,2)}[label]()
                    reference = str(value)
                    target.append(dict(question=question,label=label,reference=reference,source='supplied_curriculum',domain='physics_scaffold' if label in ('force','speed','kinetic') else 'math_scaffold', id=identity(question)))
    conversations = {
        'greeting': (["hello", "hi", "good morning", "hello there", "how are you", "hey", "good evening"], ["hi there", "greetings"], ["hey there", "hello friend"]),
        'identity': (["what are you", "who are you", "tell me about yourself", "what is your name", "are you a brain"], ["describe yourself"], ["could you introduce yourself"]),
        'capabilities': (["what can you do", "what have you learned", "can you do math", "can you solve physics", "can we talk"], ["what can I ask you"], ["which questions can you answer"]),
        'unknown': (["write a novel", "why is the sky blue", "explain relativity", "tell me a joke", "what is the capital of France", "do you have subjective experience", "are you conscious", "what is consciousness", "can you feel pain"], ["explain quantum gravity", "do you dream"], ["what is dark matter", "do you have feelings"]),
    }
    for label, splits in conversations.items():
        for target, questions in zip((train, dev, test), splits):
            for question in questions:
                target.append(dict(question=question,label=label,source='supplied_curriculum',domain='limited_dialogue',id=identity(question)))
    dolly = args.root/'04_conversation/dolly-15k/records.jsonl.gz'
    negatives = []
    for line in gzip.open(dolly, 'rt'):
        row = json.loads(line); d = row['data']
        if not d.get('context') and len(d['instruction']) < 400:
            q = d['instruction']; negatives.append(dict(question=q,label='unknown',source='dolly-15k',domain='general_english',id=identity(q)))
    negatives.sort(key=lambda r:r['id'])
    train += [r for r in negatives if int(r['id'][:8],16)%5 != 0][:100]
    broad += [r for r in negatives if int(r['id'][:8],16)%5 == 0][:100]
    physics = args.root/'06_physics/scibench/records.jsonl.gz'
    for line in gzip.open(physics, 'rt'):
        row=json.loads(line); d=row['data']; q=d.get('problem_text','')
        if q and len(q)<8192:
            broad.append(dict(question=q,reference=str(d.get('answer_number',d.get('answer_latex',''))),source='scibench_evaluation_only',domain='scibench',id=identity(q)))
    training_ids={r['id'] for r in train}
    dev=[r for r in dev if r['id'] not in training_ids]
    test=[r for r in test if r['id'] not in training_ids]
    broad=[r for r in broad if r['id'] not in training_ids]
    dev_ids={r['id'] for r in dev}
    evaluation_ids={r['id'] for r in test+broad}
    assert not training_ids & dev_ids
    assert not training_ids & evaluation_ids
    assert not dev_ids & evaluation_ids
    args.output.mkdir(parents=True,exist_ok=True)
    for name, rows in [('train',train),('dev',dev),('test',test),('broad',broad)]:
        with (args.output/f'{name}.jsonl').open('w') as f:
            for r in rows:f.write(json.dumps(r)+'\n')
    manifest=dict(dataset_root=str(args.root.resolve()),catalog=catalog,counts={name:dict(collections.Counter(r['domain'] for r in rows)) for name,rows in [('train',train),('dev',dev),('test',test),('broad',broad)]},source_sha256={str(p.relative_to(args.root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [source,dolly,physics]},scope='GSM programs inferred from training worked solutions only. Supplied English/physics scaffolds are separate from real upstream generalization. SciBench is evaluation only. No pretrained model.')
    (args.output/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    print(json.dumps(manifest['counts'],indent=2))

if __name__=='__main__':main()
