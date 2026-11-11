import random, sys

OSOME = 'Some("o" + "")'
TYN = {'Int':'Int','Str':'Str','R':'R','P':'P','N':'N','OR':'Option<R>','LR':'List<R>','OS':'Option<Str>'}

class G:
    def __init__(self, seed):
        self.r = random.Random(seed)
        self.n = 0
        self.inhelper = False
    def name(self, p='v'):
        self.n += 1
        return f'{p}{self.n}'
    def pick(self, xs): return self.r.choice(xs)
    def locals_of(self, scope, t): return [v for v,(tt,m) in scope.items() if tt == t]

    def e(self, t, scope, d):
        x = self.e0(t, scope, d)
        return x if x[0] in '("[' or x.isidentifier() or x.isdigit() else '(' + x + ')'

    def e0(self, t, scope, d):
        r = self.r
        if d <= 0:
            return self.leaf(t, scope)
        if t == 'Int':
            k = r.randint(0, 9)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'{self.e("R", scope, d-1)}.n'
            if k == 2: return f'{self.e("R", scope, d-1)}.xs.len()'
            if k == 3: return f'{self.e("P", scope, d-1)}.x'
            if k == 4: return f'{self.e("N", scope, d-1)}.p.y'
            if k == 5: return f'({self.e("Int", scope, d-1)} + {self.e("Int", scope, d-1)})'
            if k == 6: return f'{self.e("Str", scope, d-1)}.len()'
            if k == 7: return f'{self.e("LR", scope, d-1)}.len()'
            if k == 8: return f'{self.e("N", scope, d-1)}.r.n'
            return f'match {self.e("OR", scope, d-1)} {{ Some(q) => q.n, None => {r.randint(-3,3)} }}'
        if t == 'Str':
            k = r.randint(0, 11)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'{self.e("R", scope, d-1)}.name'
            if k == 2: return f'{self.e("N", scope, d-1)}.s'
            if k == 3: return f'{self.e("N", scope, d-1)}.o.unwrap_or({self.e("Str", scope, d-1)})'
            if k == 4: return f'({self.e("Str", scope, d-1)} + {self.e("Str", scope, d-1)})'
            if k == 5: return f'{self.e("Int", scope, d-1)}.to_str()'
            if k == 6: return f'show({self.e("R", scope, d-1)})'
            if k == 7: return f'{self.e("N", scope, d-1)}.r.name'
            if k == 8: return f'({self.e("R", scope, d-1)} == {self.e("R", scope, d-1)}).to_str()'
            if k == 9: return f'shown({self.e("N", scope, d-1)})'
            if k == 10 and not self.inhelper: return f'h_str({self.e("R", scope, d-1)}, {self.e("R", scope, d-1)})'
            return f'match {self.e("OR", scope, d-1)} {{ Some(q) => q.name, None => {self.e("Str", scope, d-1)} }}'
        if t == 'R':
            k = r.randint(0, 9)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'R {{ name: {self.e("Str", scope, d-1)}, xs: [{self.e("Int", scope, d-1)}], n: {self.e("Int", scope, d-1)} }}'
            if k == 2: return f'mk_r({self.e("Str", scope, d-1)}, {self.e("Int", scope, d-1)})'
            if k == 3 and not self.inhelper: return f'h_r({self.e("R", scope, d-1)}, {self.e("Int", scope, d-1)})'
            if k == 4: return f'R {{ name: {self.e("Str", scope, d-1)}, ..{self.e("R", scope, d-1)} }}'
            if k == 5: return f'if {self.cond(scope, d-1)} {{ {self.e("R", scope, d-1)} }} else {{ {self.e("R", scope, d-1)} }}'
            if k == 6: return f'match {self.e("OR", scope, d-1)} {{ Some(q) => q, None => {self.e("R", scope, d-1)} }}'
            if k == 7: return f'{self.e("N", scope, d-1)}.r'
            if k == 8: return f'{self.e("OR", scope, d-1)}.unwrap_or({self.e("R", scope, d-1)})'
            return f'R {{ n: {self.e("Int", scope, d-1)}, xs: {self.e("R", scope, d-1)}.xs, ..{self.e("R", scope, d-1)} }}'
        if t == 'P':
            k = r.randint(0, 4)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'P {{ x: {self.e("Int", scope, d-1)}, y: {self.e("Int", scope, d-1)} }}'
            if k == 2: return f'{self.e("N", scope, d-1)}.p'
            if k == 3 and not self.inhelper: return f'h_p({self.e("P", scope, d-1)})'
            return f'if {self.cond(scope, d-1)} {{ {self.e("P", scope, d-1)} }} else {{ {self.e("P", scope, d-1)} }}'
        if t == 'N':
            k = r.randint(0, 4)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'N {{ r: {self.e("R", scope, d-1)}, p: {self.e("P", scope, d-1)}, o: {self.e("OS", scope, d-1)}, s: {self.e("Str", scope, d-1)} }}'
            if k == 2 and not self.inhelper: return f'h_n({self.e("N", scope, d-1)})'
            if k == 3: return f'N {{ r: {self.e("R", scope, d-1)}, ..{self.e("N", scope, d-1)} }}'
            return f'if {self.cond(scope, d-1)} {{ {self.e("N", scope, d-1)} }} else {{ {self.e("N", scope, d-1)} }}'
        if t == 'OR':
            k = r.randint(0, 5)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'mk_some({self.e("R", scope, d-1)})'
            if k == 2: return f'first({self.e("LR", scope, d-1)})'
            if k == 3: return f'if {self.cond(scope, d-1)} {{ {self.e("OR", scope, d-1)} }} else {{ {self.e("OR", scope, d-1)} }}'
            if k == 4: return f'{self.e("OR", scope, d-1)}.map(|q: R| R {{ name: q.name + "m", ..q }})'
            return f'{self.e("LR", scope, d-1)}.pop()' if False else f'first({self.e("LR", scope, d-1)})'
        if t == 'OS':
            k = r.randint(0, 3)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'Some({self.e("Str", scope, d-1)})'
            if k == 2: return f'{self.e("N", scope, d-1)}.o'
            return f'{self.e("OR", scope, d-1)}.map(|q: R| q.name)'
        if t == 'LR':
            k = r.randint(0, 5)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'[{self.e("R", scope, d-1)}, {self.e("R", scope, d-1)}]'
            if k == 2: return f'{self.e("LR", scope, d-1)}.map(|q: R| mk_r(q.name + "x", q.n + 1))'
            if k == 3: return f'{self.e("LR", scope, d-1)}.filter(|q: R| q.n % 2 == 0)'
            if k == 4: return f'[{self.e("N", scope, d-1)}.r]'
            return f'if {self.cond(scope, d-1)} {{ {self.e("LR", scope, d-1)} }} else {{ {self.e("LR", scope, d-1)} }}'
        raise Exception(t)

    def cond(self, scope, d):
        k = self.r.randint(0, 4)
        if k == 0: return f'{self.e("OR", scope, d)}.is_some()'
        if k == 1: return f'{self.e("Int", scope, d)} % 2 == 0'
        if k == 2: return f'{self.e("R", scope, d)} == {self.e("R", scope, d)}'
        if k == 3: return f'{self.e("N", scope, d)} != {self.e("N", scope, d)}'
        return f'{self.e("Str", scope, d)}.len() > {self.r.randint(0, 5)}'

    def leaf(self, t, scope):
        r = self.r
        ls = self.locals_of(scope, t)
        if ls and r.random() < 0.65: return self.pick(ls)
        if t == 'Int': return str(r.randint(-5, 9))
        if t == 'Str': return f'("{self.pick(["a","bb","ccc","","dd"])}" + "")'
        if t == 'R': return f'R {{ name: "{self.pick(["r","s",""])}" + "", xs: [{r.randint(0,3)}], n: {r.randint(-2,5)} }}'
        if t == 'P': return f'P {{ x: {r.randint(-2,5)}, y: {r.randint(-2,5)} }}'
        if t == 'N': return f'N {{ r: {self.leaf("R", scope)}, p: {self.leaf("P", scope)}, o: {self.pick(["None", OSOME])}, s: "n" + "" }}'
        if t == 'OR': return self.pick(['mk_none()', f'mk_some({self.leaf("R", scope)})'])
        if t == 'OS': return self.pick(['None', 'Some("q" + "")'])
        if t == 'LR': return self.pick(['mk_empty()', f'[{self.leaf("R", scope)}]'])

    def stmts(self, scope, d, inloop, n):
        return [self.stmt(scope, d, inloop) for _ in range(n)]

    def stmt(self, scope, d, inloop):
        r = self.r
        k = r.randint(0, 13)
        if k <= 2:
            t = self.pick(['Int','Str','R','R','P','N','N','OR','LR','OS'])
            v = self.name()
            mut = r.random() < 0.6
            init = self.e(t, scope, d)
            scope[v] = (t, mut)
            return f'let {"mut " if mut else ""}{v}: {TYN[t]} = {init};'
        if k == 3:
            muts = [v for v,(t,m) in scope.items() if m]
            if muts:
                v = self.pick(muts); t = scope[v][0]
                return f'{v} = {self.e(t, scope, d)};'
            return f'out = out + {self.e("Str", scope, d)};'
        if k == 4 or k == 5:
            # field writes
            muts = [v for v,(t,m) in scope.items() if m and t in ('R','P','N','LR')]
            if not muts: return f'out = out + {self.e("Str", scope, d)};'
            v = self.pick(muts); t = scope[v][0]
            if t == 'R':
                return self.pick([
                    f'{v}.name = {self.e("Str", scope, d)};',
                    f'{v}.name += {self.e("Str", scope, d)};',
                    f'{v}.n += {self.e("Int", scope, d)};',
                    f'{v}.n = {self.e("Int", scope, d)};',
                    f'{v}.xs.push({self.e("Int", scope, d)});',
                    f'{v}.xs = [{self.e("Int", scope, d)}];',
                    f'{v}.name = {v}.name + {v}.name;',
                    f'{v}.xs.push({v}.xs.len());',
                ])
            if t == 'P':
                return self.pick([f'{v}.x = {self.e("Int", scope, d)};', f'{v}.y += {self.e("Int", scope, d)};', f'{v}.x += {v}.y;'])
            if t == 'N':
                return self.pick([
                    f'{v}.r.name += {self.e("Str", scope, d)};',
                    f'{v}.r.xs.push({self.e("Int", scope, d)});',
                    f'{v}.r = {self.e("R", scope, d)};',
                    f'{v}.p.x = {self.e("Int", scope, d)};',
                    f'{v}.p = {self.e("P", scope, d)};',
                    f'{v}.o = {self.e("OS", scope, d)};',
                    f'{v}.s += {self.e("Str", scope, d)};',
                    f'{v}.r.n += {v}.p.x;',
                    f'{v}.s = {v}.r.name;',
                ])
            if t == 'LR':
                return self.pick([
                    f'{v}.push({self.e("R", scope, d)});',
                    f'if {v}.len() > 0 {{ {v}[0].name += {self.e("Str", scope, d)}; }}',
                    f'if {v}.len() > 0 {{ {v}[0].xs.push({self.e("Int", scope, d)}); }}',
                    f'if {v}.len() > 0 {{ {v}[0] = {self.e("R", scope, d)}; }}',
                    f'match {v}.pop() {{ Some(q) => {{ out = out + show(q); }} None => {{ out = out + "pe"; }} }}',
                ])
        if k == 6:
            return f'out = out + {self.e("Str", scope, d)};'
        if k == 7:
            inner = dict(scope)
            body = self.stmts(inner, d-1, inloop, r.randint(1,3))
            inner2 = dict(scope)
            body2 = self.stmts(inner2, d-1, inloop, r.randint(1,2))
            return f'if {self.cond(scope, d-1)} {{ ' + ' '.join(body) + ' } else { ' + ' '.join(body2) + ' }'
        if k == 8 and d > 0:
            i = self.name('i')
            inner = dict(scope); inner[i] = ('Int', False)
            body = self.stmts(inner, d-1, True, r.randint(1,3))
            ex = ''
            if r.random() < 0.6:
                ex = f' if {self.cond(inner, 0)} {{ {self.pick(["break;","continue;"])} }}'
            return f'for {i} in 0..{r.randint(1,3)} {{ ' + ' '.join(body) + ex + ' }'
        if k == 9 and d > 0:
            q = self.name('q')
            inner = dict(scope); inner[q] = ('R', False)
            body = self.stmts(inner, d-1, True, r.randint(1,3))
            ex = ''
            if r.random() < 0.5:
                ex = f' if {self.cond(inner, 0)} {{ {self.pick(["break;","continue;"])} }}'
            return f'for {q} in {self.e("LR", scope, d-1)} {{ ' + ' '.join(body) + ex + ' }'
        if k == 10:
            q = self.name('q'); inner = dict(scope); inner[q] = ('R', False)
            arm1 = ' '.join(self.stmts(inner, d-1, inloop, r.randint(1,2)))
            arm2 = ' '.join(self.stmts(dict(scope), d-1, inloop, 1))
            return f'match {self.e("OR", scope, d-1)} {{ Some({q}) => {{ {arm1} }} None => {{ {arm2} }} }}'
        if k == 11:
            # closure capturing locals, called at once and later
            f = self.name('f')
            scope[f] = ('F', False)
            return f'let {f} = |z: Int| {self.e("Str", scope, d-1)} + z.to_str() + {self.e("R", scope, d-1)}.name;'
        if k == 12:
            fs = self.locals_of(scope, 'F')
            if fs: return f'out = out + {self.pick(fs)}({self.e("Int", scope, 0)});'
            return f'out = out + {self.e("Str", scope, d)};'
        if k == 13 and inloop and r.random() < 0.3:
            return f'if {self.cond(scope, 0)} {{ {self.pick(["break;", "continue;"])} }}'
        return f'out = out + {self.e("Str", scope, d)};'

    def func(self, name, params, ret):
        self.inhelper = True
        scope = {p: (t, False) for p, t in params}
        ps = ', '.join(f'{p}: {TYN[t]}' for p, t in params)
        body = self.stmts(scope, 2, False, self.r.randint(2, 5))
        tail = self.e(ret, scope, 2)
        pre = 'let mut out: Str = "" + "";'
        if ret == 'R': tail = f'R {{ name: {tail}.name + out, ..{tail} }}'
        elif ret == 'N': tail = f'N {{ s: {tail}.s + out, ..{tail} }}'
        elif ret == 'Str': tail = f'{tail} + out'
        elif ret == 'P': tail = f'P {{ x: {tail}.x + out.len(), y: {tail}.y }}'
        self.inhelper = False
        return f'fn {name}({ps}) -> {TYN[ret]} {{\n    {pre}\n    ' + '\n    '.join(body) + f'\n    {tail}\n}}\n'

def program(seed):
    g = G(seed)
    src = 'struct R { name: Str, xs: List<Int>, n: Int }\nstruct P { x: Int, y: Int }\nstruct N { r: R, p: P, o: Option<Str>, s: Str }\n'
    src += 'fn mk_r(s: Str, n: Int) -> R { R { name: s, xs: [n, n], n: n } }\nfn mk_some(r: R) -> Option<R> { Some(r) }\nfn mk_none() -> Option<R> { None }\nfn mk_empty() -> List<R> { [] }\n'
    src += 'fn first(xs: List<R>) -> Option<R> { if xs.len() > 0 { Some(xs[0]) } else { None } }\n'
    src += 'fn show(r: R) -> Str { r.name + ":" + r.xs.len().to_str() + ":" + r.n.to_str() + ";" }\n'
    src += 'fn shown(n: N) -> Str { show(n.r) + n.p.x.to_str() + n.p.y.to_str() + n.o.unwrap_or("_") + n.s + ";" }\n'
    src += g.func('h_r', [('a','R'),('k','Int')], 'R')
    src += g.func('h_p', [('a','P')], 'P')
    src += g.func('h_n', [('a','N')], 'N')
    src += g.func('h_str', [('a','R'),('b','R')], 'Str')
    scope = {}
    body = g.stmts(scope, 3, False, g.r.randint(6, 14))
    src += 'fn main() -> Str {\n    let mut out: Str = "" + "";\n    ' + '\n    '.join(body) + '\n    out\n}\n'
    return src

if __name__ == '__main__':
    first = int(sys.argv[1]); n = int(sys.argv[2]); outdir = sys.argv[3]
    for s in range(first, first + n):
        open(f'{outdir}/s{s}.kio', 'w').write(program(s))
