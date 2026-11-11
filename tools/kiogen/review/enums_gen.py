import random, sys

# Types: 'Int','Str','OS' (Option<Str>),'RS' (Result<Str, Int>),'J','OOS' (Option<Option<Str>>)
TYN = {'Int':'Int','Str':'Str','OS':'Option<Str>','RS':'Result<Str, Int>','J':'J','OOS':'Option<Option<Str>>'}

class G:
    def __init__(self, seed):
        self.r = random.Random(seed)
        self.n = 0
        self.lines = []
        self.inhelper = False
    def name(self, p='v'):
        self.n += 1
        return f'{p}{self.n}'
    def pick(self, xs): return self.r.choice(xs)
    def locals_of(self, scope, t): return [v for v,(tt,m) in scope.items() if tt == t]

    def e(self, t, scope, d, fn):
        x = self.e0(t, scope, d, fn)
        return x if x[0] in '("' or x.isidentifier() or x.isdigit() else '(' + x + ')'

    def e0(self, t, scope, d, fn):
        r = self.r
        if d <= 0:
            return self.leaf(t, scope)
        c = r.random()
        if t == 'Int':
            k = r.randint(0, 6)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'({self.e("Str", scope, d-1, fn)}).len()'
            if k == 2: return f'{self.e("OS", scope, d-1, fn)}.map(|s: Str| s.len()).unwrap_or({r.randint(-3,3)})'
            if k == 3: return f'match {self.e("RS", scope, d-1, fn)} {{ Ok(s) => s.len(), Err(n) => n }}'
            if k == 4: return f'({self.e("Int", scope, d-1, fn)} + {self.e("Int", scope, d-1, fn)})'
            if k == 5: return f'match {self.e("J", scope, d-1, fn)} {{ J::Null => 0, J::N(n) => n, J::S(s) => s.len() }}'
            return f'{self.e("OOS", scope, d-1, fn)}.map(|o: Option<Str>| o.map(|s: Str| s.len()).unwrap_or(-1)).unwrap_or(-2)'
        if t == 'Str':
            k = r.randint(0, 8)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'({self.e("Str", scope, d-1, fn)} + {self.e("Str", scope, d-1, fn)})'
            if k == 2: return f'{self.e("OS", scope, d-1, fn)}.unwrap_or({self.e("Str", scope, d-1, fn)})'
            if k == 3: return f'{self.e("RS", scope, d-1, fn)}.unwrap_or({self.e("Str", scope, d-1, fn)})'
            if k == 4: return f'match {self.e("J", scope, d-1, fn)} {{ J::Null => "nul", J::N(n) => n.to_str(), J::S(s) => s }}'
            if k == 5: return f'{self.e("Int", scope, d-1, fn)}.to_str()'
            if k == 6: return f'match {self.e("OOS", scope, d-1, fn)} {{ Some(Some(s)) => s, Some(None) => "sn", None => "n" }}'
            if k == 7: return f'{self.e("OS", scope, d-1, fn)}.unwrap_or_else(|| {self.e("Str", scope, d-1, fn)})'
            return f'{self.e("RS", scope, d-1, fn)}.unwrap_or_else(|n: Int| n.to_str())'
        if t == 'OS':
            k = r.randint(0, 11)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'mk_some({self.e("Str", scope, d-1, fn)})'
            if k == 2: return f'{self.e("OS", scope, d-1, fn)}.map(|s: Str| s + {self.e("Str", scope, d-1, fn)})'
            if k == 3: return f'{self.e("OS", scope, d-1, fn)}.and_then(|s: Str| if s.len() > {r.randint(0,6)} {{ Some(s) }} else {{ None }})'
            if k == 4: return f'{self.e("RS", scope, d-1, fn)}.ok()'
            if k == 5: return f'({self.e("Str", scope, d-1, fn)}).strip_prefix({self.e("Str", scope, d-1, fn)})'
            if k == 6: return f'if {self.cond(scope, d-1, fn)} {{ {self.e("OS", scope, d-1, fn)} }} else {{ {self.e("OS", scope, d-1, fn)} }}'
            if k == 7: return f'{self.e("OOS", scope, d-1, fn)}.unwrap_or({self.e("OS", scope, d-1, fn)})'
            if k == 8: return f'match {self.e("J", scope, d-1, fn)} {{ J::Null => None, J::N(n) => Some(n.to_str()), J::S(s) => Some(s) }}'
            if k == 9: return f'{self.e("OOS", scope, d-1, fn)}.and_then(|o: Option<Str>| o)'
            if k == 10 and not self.inhelper: return f'h_os({self.e("OS", scope, d-1, fn)}, {self.e("Int", scope, d-1, fn)})'
            return f'{self.e("RS", scope, d-1, fn)}.err().map(|n: Int| n.to_str())'
        if t == 'RS':
            k = r.randint(0, 8)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'mk_ok({self.e("Str", scope, d-1, fn)})'
            if k == 2: return f'mk_err({self.e("Int", scope, d-1, fn)})'
            if k == 3: return f'{self.e("OS", scope, d-1, fn)}.ok_or({self.e("Int", scope, d-1, fn)})'
            if k == 4: return f'{self.e("RS", scope, d-1, fn)}.map(|s: Str| s + "m")'
            if k == 5: return f'{self.e("RS", scope, d-1, fn)}.map_err(|n: Int| n + 1)'
            if k == 6: return f'{self.e("RS", scope, d-1, fn)}.and_then(|s: Str| if s.len() % 2 == 0 {{ Ok(s + "a") }} else {{ Err(s.len()) }})'
            if k == 7 and not self.inhelper: return f'h_rs({self.e("RS", scope, d-1, fn)}, {self.e("Str", scope, d-1, fn)})'
            return f'if {self.cond(scope, d-1, fn)} {{ {self.e("RS", scope, d-1, fn)} }} else {{ {self.e("RS", scope, d-1, fn)} }}'
        if t == 'J':
            k = r.randint(0, 6)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'J::N({self.e("Int", scope, d-1, fn)})'
            if k == 2: return f'J::S({self.e("Str", scope, d-1, fn)})'
            if k == 3: return 'J::Null'
            if k == 4 and not self.inhelper: return f'h_j({self.e("J", scope, d-1, fn)}, {self.e("OS", scope, d-1, fn)})'
            if k == 5: return f'match {self.e("OS", scope, d-1, fn)} {{ Some(s) => J::S(s), None => J::N({r.randint(0,9)}) }}'
            return f'if {self.cond(scope, d-1, fn)} {{ {self.e("J", scope, d-1, fn)} }} else {{ {self.e("J", scope, d-1, fn)} }}'
        if t == 'OOS':
            k = r.randint(0, 5)
            if k == 0: return self.leaf(t, scope)
            if k == 1: return f'mk_soos({self.e("OS", scope, d-1, fn)})'
            if k == 2: return f'{self.e("OS", scope, d-1, fn)}.map(|s: Str| if s.len() > 2 {{ Some(s) }} else {{ None }})'
            if k == 3: return f'if {self.cond(scope, d-1, fn)} {{ {self.e("OOS", scope, d-1, fn)} }} else {{ {self.e("OOS", scope, d-1, fn)} }}'
            if k == 4:
                ms = self.locals_of(scope, 'MAP')
                if ms: return f'{self.pick(ms)}.get({self.e("Str", scope, d-1, fn)})'
                return f'mk_soos({self.e("OS", scope, d-1, fn)})'
            if not self.inhelper: return f'h_oos({self.e("OOS", scope, d-1, fn)})'
            return self.leaf(t, scope)
        raise Exception(t)

    def cond(self, scope, d, fn):
        k = self.r.randint(0, 4)
        if k == 0: return f'{self.e("OS", scope, d, fn)}.is_some()'
        if k == 1: return f'{self.e("RS", scope, d, fn)}.is_err()'
        if k == 2: return f'{self.e("Int", scope, d, fn)} % 2 == 0'
        if k == 3: return f'{self.e("OS", scope, d, fn)} == {self.e("OS", scope, d, fn)}'
        return f'{self.e("J", scope, d, fn)} != {self.e("J", scope, d, fn)}'

    def leaf(self, t, scope):
        r = self.r
        ls = self.locals_of(scope, t)
        if ls and r.random() < 0.6: return self.pick(ls)
        if t == 'Int': return str(r.randint(-5, 9))
        if t == 'Str': return f'("{self.pick(["a","bb","ccc","","dd"])}" + "")'
        if t == 'OS': return self.pick(['mk_none()', f'mk_some("{self.pick(["x","yy",""])}" + "")'])
        if t == 'RS': return self.pick(['mk_err(3)', f'mk_ok("{self.pick(["o","pp"])}" + "")'])
        if t == 'J': return self.pick(['J::Null', 'J::N(2)', 'J::S("js" + "")'])
        if t == 'OOS': return self.pick(['mk_noos()', 'mk_soos(mk_none())', 'mk_soos(mk_some("q" + ""))'])

    def stmts(self, scope, d, fn, inloop, n):
        out = []
        for _ in range(n):
            out.append(self.stmt(scope, d, fn, inloop))
        return out

    def stmt(self, scope, d, fn, inloop):
        r = self.r
        k = r.randint(0, 11)
        if k <= 2:
            t = self.pick(['Int','Str','OS','RS','J','OOS'])
            v = self.name()
            mut = r.random() < 0.5
            init = self.e(t, scope, d, fn)
            scope[v] = (t, mut)
            return f'let {"mut " if mut else ""}{v}: {TYN[t]} = {init};'
        if k == 3:
            muts = [v for v,(t,m) in scope.items() if m and t in TYN]
            if muts:
                v = self.pick(muts); t = scope[v][0]
                return f'{v} = {self.e(t, scope, d, fn)};'
            return f'out = out + {self.e("Str", scope, d, fn)};'
        if k == 4:
            return f'out = out + {self.e("Str", scope, d, fn)};'
        if k == 5:
            inner = dict(scope)
            body = self.stmts(inner, d-1, fn, inloop, r.randint(1,3))
            inner2 = dict(scope)
            body2 = self.stmts(inner2, d-1, fn, inloop, r.randint(1,2))
            return f'if {self.cond(scope, d-1, fn)} {{ ' + ' '.join(body) + ' } else { ' + ' '.join(body2) + ' }'
        if k == 6 and d > 0:
            i = self.name('i')
            inner = dict(scope); inner[i] = ('Int', False)
            body = self.stmts(inner, d-1, fn, True, r.randint(1,3))
            ex = ''
            if r.random() < 0.6:
                ex = f' if {self.cond(inner, 0, fn)} {{ {self.pick(["break;","continue;"])} }}'
            return f'for {i} in 0..{r.randint(1,4)} {{ ' + ' '.join(body) + ex + ' }'
        if k == 7:
            t = self.pick(['OS','RS','J','OOS'])
            scrut = self.e(t, scope, d-1, fn)
            arms = []
            if t == 'OS':
                s = self.name('s'); inner = dict(scope); inner[s] = ('Str', False)
                arms.append(f'Some({s}) => {{ ' + ' '.join(self.stmts(inner, d-1, fn, inloop, r.randint(1,2))) + ' }')
                arms.append('None => { ' + ' '.join(self.stmts(dict(scope), d-1, fn, inloop, 1)) + ' }')
            elif t == 'RS':
                s = self.name('s'); inner = dict(scope); inner[s] = ('Str', False)
                arms.append(f'Ok({s}) => {{ ' + ' '.join(self.stmts(inner, d-1, fn, inloop, r.randint(1,2))) + ' }')
                n_ = self.name('n'); inner = dict(scope); inner[n_] = ('Int', False)
                arms.append(f'Err({n_}) => {{ ' + ' '.join(self.stmts(inner, d-1, fn, inloop, 1)) + ' }')
            elif t == 'J':
                arms.append('J::Null => { ' + ' '.join(self.stmts(dict(scope), d-1, fn, inloop, 1)) + ' }')
                n_ = self.name('n'); inner = dict(scope); inner[n_] = ('Int', False)
                arms.append(f'J::N({n_}) => {{ ' + ' '.join(self.stmts(inner, d-1, fn, inloop, 1)) + ' }')
                s = self.name('s'); inner = dict(scope); inner[s] = ('Str', False)
                arms.append(f'J::S({s}) => {{ ' + ' '.join(self.stmts(inner, d-1, fn, inloop, r.randint(1,2))) + ' }')
            else:
                s = self.name('s'); inner = dict(scope); inner[s] = ('Str', False)
                arms.append(f'Some(Some({s})) => {{ ' + ' '.join(self.stmts(inner, d-1, fn, inloop, r.randint(1,2))) + ' }')
                arms.append('Some(None) => { ' + ' '.join(self.stmts(dict(scope), d-1, fn, inloop, 1)) + ' }')
                arms.append('None => { ' + ' '.join(self.stmts(dict(scope), d-1, fn, inloop, 1)) + ' }')
            return f'match {scrut} {{ ' + ' '.join(arms) + ' }'
        if k == 8:
            lists = self.locals_of(scope, 'LIST')
            if lists:
                return f'{self.pick(lists)}.push({self.e("OS", scope, d, fn)});'
            v = self.name('xs'); init = f'[{self.e("OS", scope, d, fn)}, {self.e("OS", scope, d, fn)}]'; scope[v] = ('LIST', True)
            return f'let mut {v}: List<Option<Str>> = {init};'
        if k == 9:
            maps = self.locals_of(scope, 'MAP')
            if maps:
                return f'{self.pick(maps)}.insert({self.e("Str", scope, 0, fn)}, {self.e("OS", scope, d, fn)});'
            v = self.name('m'); scope[v] = ('MAP', True)
            return f'let mut {v}: Map<Str, Option<Str>> = Map::new();'
        if k == 10:
            lists = self.locals_of(scope, 'LIST')
            if lists:
                l = self.pick(lists)
                return f'match {l}.pop() {{ Some(Some(s)) => {{ out = out + s; }} Some(None) => {{ out = out + "pn"; }} None => {{ out = out + "pe"; }} }}'
            return f'out = out + {self.e("Str", scope, d, fn)};'
        if k == 11 and fn in ('OS','RS') and r.random() < 0.5:
            v = self.name('q'); init = self.e(fn, scope, d, fn); scope[v] = ('Str', False)
            return f'let {v}: Str = {init}?;'
        if k == 11 and inloop and r.random() < 0.3:
            return f'if {self.cond(scope, 0, fn)} {{ {self.pick(["break;", "continue;"])} }}'
        return f'out = out + {self.e("Str", scope, d, fn)};'

    def func(self, name, params, ret):
        self.inhelper = True
        scope = {p: (t, False) for p, t in params}
        ps = ', '.join(f'{p}: {TYN[t]}' for p, t in params)
        body = self.stmts(scope, 2, ret, False, self.r.randint(2, 5))
        tail = self.e(ret, scope, 2, ret)
        pre = 'let mut out: Str = "" + "";'
        if ret == 'OS':
            tail = f'{tail}.map(|s: Str| s + out)'
        elif ret == 'RS':
            tail = f'{tail}.map(|s: Str| s + out)'
        elif ret == 'J':
            tail = f'match {tail} {{ J::S(s) => J::S(s + out), J::N(n) => J::N(n + out.len()), J::Null => J::S(out) }}'
        elif ret == 'OOS':
            tail = f'{tail}.map(|o: Option<Str>| o.map(|s: Str| s + out))'
        self.inhelper = False
        return f'fn {name}({ps}) -> {TYN[ret]} {{\n    {pre}\n    ' + '\n    '.join(body) + f'\n    {tail}\n}}\n'

def program(seed):
    g = G(seed)
    src = 'enum J { Null, N(Int), S(Str) }\n'
    src += 'fn mk_some(s: Str) -> Option<Str> { Some(s) }\nfn mk_none() -> Option<Str> { None }\nfn mk_ok(s: Str) -> Result<Str, Int> { Ok(s) }\nfn mk_err(n: Int) -> Result<Str, Int> { Err(n) }\nfn mk_soos(o: Option<Str>) -> Option<Option<Str>> { Some(o) }\nfn mk_noos() -> Option<Option<Str>> { None }\n'
    src += g.func('h_os', [('a','OS'),('k','Int')], 'OS')
    src += g.func('h_rs', [('a','RS'),('t','Str')], 'RS')
    src += g.func('h_j', [('a','J'),('o','OS')], 'J')
    src += g.func('h_oos', [('a','OOS')], 'OOS')
    scope = {}
    body = g.stmts(scope, 3, 'Str', False, g.r.randint(6, 14))
    src += 'fn main() -> Str {\n    let mut out: Str = "" + "";\n    ' + '\n    '.join(body) + '\n    out\n}\n'
    return src

if __name__ == '__main__':
    first = int(sys.argv[1]); n = int(sys.argv[2]); outdir = sys.argv[3]
    for s in range(first, first + n):
        open(f'{outdir}/g{s}.kio', 'w').write(program(s))
