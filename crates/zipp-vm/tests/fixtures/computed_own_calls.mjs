const results = [];
function add(v) { this.count += v; return this.count; }
function dbl(v) { this.count += v * 2; return this.count; }
{
  const a = { count: 0, a: add, b: dbl };
  const b = { b: dbl, count: 0, a: add };
  let sum = 0;
  for (let i = 0; i < 16000; ++i) {
    const o = (i & 1) ? a : b;
    const k = (i % 3) ? 'a' : 'b';
    sum = (sum + o[k](i & 7)) | 0;
  }
  results.push(['own-shapes', sum, a.count, b.count]);
}
{
  const own = { count: 2, f: add, g: dbl };
  let getter = 0, setter = 0, sum = 0, errors = 0;
  const proto = { g: function(x) { this.count += x + 7; return this.count; } };
  for (let i = 0; i < 12000; ++i) {
    if (i === 3000) Object.defineProperty(own, 'f', {
      configurable: true,
      get() { getter++; return add; },
      set(v) { setter++; }
    });
    if (i === 5500) Object.defineProperty(own, 'f', { value: add, writable: true, configurable: true });
    if (i === 7000) { delete own.g; Object.setPrototypeOf(own, proto); }
    if (i === 9500) Object.defineProperty(own, 'g', { value: dbl, writable: true, configurable: true });
    const k = i & 1 ? 'f' : 'g';
    try { sum = (sum + own[k](i & 3)) | 0; } catch(e) { errors++; }
  }
  results.push(['mutations', sum, own.count, getter, setter, errors]);
}
{
  class Box {
    constructor(start) { this.count=start; this.fun=add; }
  }
  const a = new Box(3), b = new Box(9);
  let sum=0;
  for (let i=0;i<12000;i++) { const obj = i & 1 ? a : b; sum=(sum+obj['fun'](1))|0; }
  results.push(['class-instance', sum, a.count, b.count]);
}
{
  let traps=0; const target={count:0, foo:add};
  const prox=new Proxy(target,{get(obj,k,recv) { traps++; return Reflect.get(obj,k,recv); }});
  let sum=0;
  for(let i=0;i<3000;i++) sum=(sum+prox['foo'](2))|0;
  results.push(['proxy',sum,traps,target.count]);
}
{
  const key=Symbol('f');
  const receiver={count:0,[key]:add};
  let sum=0;
  for(let i=0;i<3000;i++)sum=(sum+receiver[key](1))|0;
  results.push(['symbol',sum,receiver.count]);
}
{
  let called=0, thrown=0;
  const obj={ boom:function(v){called++; if(v%3===0)throw Error('expected');return v;} };
  let sum=0;
  for(let i=0;i<6000;i++) {
    try {sum=(sum+obj['boom'](i))|0;} catch(e) { thrown++; }
  }
  results.push(['throw-once',sum,called,thrown]);
}
{
  const obj={count:1, h:add};
  const key='h';
  let sum=0;
  for(let i=0;i<4000;i++) {
    let dynamicKey=''; dynamicKey+=key;
    sum=(sum+obj[dynamicKey](1))|0;
  }
  results.push(['dynamic-key',sum,obj.count]);
}
console.log(JSON.stringify(results));
