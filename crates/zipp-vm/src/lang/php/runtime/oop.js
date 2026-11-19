/* ZIPP PHP runtime — OOP (classes, interfaces, traits, enums, visibility, LSB, magic,
 * type checking, exceptions, closures, generators, reflection, autoloading). Apache-2.0. */
(function (R) {
    "use strict";
    const rt = R.__rt;

    const classes = new Map();     // canonical lowercase fqn -> ClassMeta
    const autoloaders = [];
    const destructQueue = [];      // tracked objects with __destruct for shutdown
    let nextObjId = 1;
    rt.classes = classes;
    rt.autoloaders = autoloaders;

    function canon(name) {
        if (name.startsWith("\\")) name = name.slice(1);
        return name.toLowerCase();
    }
    function stripLeading(name) {
        return name.startsWith("\\") ? name.slice(1) : name;
    }
    rt.canon = canon;

    function resolveClass(name, callerScopeCls, calledCls, autoload) {
        if (name && typeof name === "object") {
            if (name.__php === 2) return name.cls;
            if (name.isClassMeta) return name;
        }
        const s = stripLeading(rt.toStr(name));
        const lo = s.toLowerCase();
        if (lo === "self") {
            if (!callerScopeCls) rt.throwError("Error", 'Cannot access "self" when no class scope is active');
            return callerScopeCls;
        }
        if (lo === "static") {
            if (!calledCls) rt.throwError("Error", 'Cannot access "static" when no class scope is active');
            return calledCls;
        }
        if (lo === "parent") {
            if (!callerScopeCls || !callerScopeCls.parent) {
                rt.throwError("Error", 'Cannot access "parent" when current class scope has no parent');
            }
            return callerScopeCls.parent;
        }
        let cls = classes.get(lo);
        if (cls) return cls;
        if (autoload !== false && autoloaders.length > 0) {
            for (let i = 0; i < autoloaders.length; i++) {
                rt.call(autoloaders[i], [s], null, null, false);
                cls = classes.get(lo);
                if (cls) return cls;
            }
        }
        return null;
    }
    rt.resolveClass = resolveClass;

    function requireClass(name, callerScopeCls, calledCls) {
        const cls = resolveClass(name, callerScopeCls, calledCls, true);
        if (!cls) rt.throwError("Error", 'Class "' + stripLeading(rt.toStr(name)) + '" not found');
        return cls;
    }
    rt.requireClass = requireClass;

    function isSubtype(cls, target) {
        if (!cls || !target) return false;
        let cur = cls;
        while (cur) {
            if (cur === target) return true;
            if (cur.allIfaces && cur.allIfaces.has(target.canon)) return true;
            cur = cur.parent;
        }
        return false;
    }
    rt.isSubtype = isSubtype;

    // ---- Class / Interface / Trait / Enum definition ----
    function makeClassMeta(spec) {
        const name = stripLeading(spec.name);
        const c = canon(name);
        const meta = {
            isClassMeta: true,
            kind: spec.kind || "class", // "class" | "interface" | "trait" | "enum"
            name: name,
            canon: c,
            isAbstract: !!spec.isAbstract,
            isFinal: !!spec.isFinal,
            isReadonly: !!spec.isReadonly,
            file: spec.file || null,
            backingType: spec.backingType || null,
            parent: null,
            interfaces: [],
            allIfaces: new Set(),
            traits: [],
            methods: new Map(),      // lowercase -> MethodMeta
            mObj: Object.create(null),
            props: new Map(),        // name -> PropMeta (non-static)
            pObj: Object.create(null),
            propInits: [],
            hasPrivateProps: false,
            hasReadonly: !!spec.isReadonly,
            hasHooks: false,
            hasMagicProps: false,
            hasDestruct: false,
            fastProps: false,
            staticProps: new Map(),  // name -> { vis, owner, ty, readonly, ref: PhpRef }
            consts: new Map(),       // name -> { vis, owner, val, isEnumCase }
            enumCases: new Map(),    // caseName -> PhpObject
            attrs: spec.attrs || [],
        };
        if (spec.extends) {
            if (Array.isArray(spec.extends)) {
                for (const ex of spec.extends) {
                    const p = requireClass(ex, null, null);
                    meta.interfaces.push(p);
                    meta.allIfaces.add(p.canon);
                    for (const sub of p.allIfaces) meta.allIfaces.add(sub);
                    for (const [k, v] of p.consts) if (!meta.consts.has(k)) meta.consts.set(k, v);
                }
            } else {
                const p = requireClass(spec.extends, null, null);
                if (p.isFinal) rt.throwError("Error", "Class " + name + " cannot extend final class " + p.name);
                meta.parent = p;
                // Inherit parent constants, non-private methods, properties
                for (const k of p.allIfaces) meta.allIfaces.add(k);
                for (const [k, v] of p.consts) meta.consts.set(k, v);
                for (const [k, v] of p.methods) {
                    if (v.vis !== 2 || p.kind === "trait") meta.methods.set(k, v);
                }
                for (const [k, v] of p.props) meta.props.set(k, v);
                if (p.hasPrivateProps) meta.hasPrivateProps = true;
                for (const [k, v] of p.staticProps) meta.staticProps.set(k, v);
            }
        }
        if (spec.implements) {
            for (const im of spec.implements) {
                const iface = requireClass(im, null, null);
                meta.interfaces.push(iface);
                meta.allIfaces.add(iface.canon);
                for (const sub of iface.allIfaces) meta.allIfaces.add(sub);
                for (const [k, v] of iface.consts) if (!meta.consts.has(k)) meta.consts.set(k, v);
            }
        }
        // Traits flattening (including insteadof / alias adaptations)
        if (spec.traits && spec.traits.length > 0) {
            for (const tu of spec.traits) {
                const tList = tu.names.map(tn => requireClass(tn, null, null));
                for (const t of tList) {
                    meta.traits.push(t);
                    for (const [k, pm] of t.props) {
                        const newKey = pm.vis === 2 ? ("\0" + meta.name + "\0" + pm.name) : k;
                        if (!meta.props.has(newKey)) {
                            meta.props.set(newKey, Object.assign({}, pm, { key: newKey, owner: meta }));
                        }
                    }
                    if (t.hasPrivateProps) meta.hasPrivateProps = true;
                    for (const [k, sp] of t.staticProps) {
                        if (!meta.staticProps.has(k)) {
                            meta.staticProps.set(k, {
                                vis: sp.vis, owner: meta, ty: sp.ty, readonly: sp.readonly,
                                ref: rt.mkRef(sp.ref.v)
                            });
                        }
                    }
                    for (const [k, cm] of t.consts) {
                        if (!meta.consts.has(k)) meta.consts.set(k, Object.assign({}, cm, { owner: meta }));
                    }
                }
                // Apply methods respecting insteadof
                const excluded = new Set();
                if (tu.adaptations) {
                    for (const ad of tu.adaptations) {
                        if (ad.kind === "precedence") {
                            for (const ex of ad.insteadOf) excluded.add(canon(ex) + "::" + ad.method.toLowerCase());
                        }
                    }
                }
                for (const t of tList) {
                    for (const [mk, mm] of t.methods) {
                        if (excluded.has(t.canon + "::" + mk)) continue;
                        meta.methods.set(mk, Object.assign({}, mm, { owner: meta }));
                    }
                }
                if (tu.adaptations) {
                    for (const ad of tu.adaptations) {
                        if (ad.kind === "alias") {
                            const mk = ad.method.toLowerCase();
                            const srcTrait = ad.trait ? requireClass(ad.trait, null, null) : tList.find(t => t.methods.has(mk));
                            if (srcTrait && srcTrait.methods.has(mk)) {
                                const orig = srcTrait.methods.get(mk);
                                const targetName = (ad.alias || ad.method).toLowerCase();
                                meta.methods.set(targetName, Object.assign({}, orig, {
                                    name: ad.alias || orig.name,
                                    vis: ad.vis !== null && ad.vis !== undefined ? ad.vis : orig.vis,
                                    owner: meta
                                }));
                            }
                        }
                    }
                }
            }
        }
        // Register class early so `self::` / `ClassName::` inside own const/prop initializers resolve!
        classes.set(c, meta);

        // Own constants (support deferred `valFn` so forward & `self::` const references work)
        if (spec.consts) {
            for (const item of spec.consts) {
                if (typeof item.valFn === "function") {
                    meta.consts.set(item.name, { vis: item.vis || 0, owner: meta, valFn: item.valFn, resolved: false, val: null });
                } else {
                    meta.consts.set(item.name, { vis: item.vis || 0, owner: meta, resolved: true, val: rt.share(rt.deref(item.val)) });
                }
            }
            for (const item of spec.consts) {
                const entry = meta.consts.get(item.name);
                if (entry && !entry.resolved && typeof entry.valFn === "function") {
                    entry.resolved = true;
                    entry.val = rt.share(rt.deref(entry.valFn.call(null, [], null, meta, null)));
                }
            }
        }
        // Own properties (support deferred `defaultFn` for `[self::CONST => ...]` / `new Foo()` defaults!)
        if (spec.props) {
            for (const p of spec.props) {
                const evalDef = () => typeof p.defaultFn === "function"
                    ? rt.share(rt.deref(p.defaultFn.call(null, [], null, meta, null)))
                    : p.defaultVal;
                if (p.isStatic) {
                    meta.staticProps.set(p.name, {
                        vis: p.vis || 0, owner: meta, ty: p.ty || null, readonly: !!p.readonly,
                        ref: rt.mkRef(p.hasDefault ? evalDef() : null),
                        init: p.hasDefault
                    });
                } else {
                    const key = p.vis === 2 ? ("\0" + meta.name + "\0" + p.name) : p.name;
                    if (p.vis === 2) meta.hasPrivateProps = true;
                    meta.props.set(key, {
                        name: p.name, key: key, vis: p.vis || 0, owner: meta,
                        setVis: p.setVis !== undefined ? p.setVis : (p.vis || 0),
                        ty: p.ty || null, readonly: !!(p.readonly || meta.isReadonly),
                        promoted: !!p.promoted,
                        hasDefault: !!p.hasDefault,
                        defaultVal: p.hasDefault && typeof p.defaultFn !== "function" ? p.defaultVal : undefined,
                        defaultFn: typeof p.defaultFn === "function" ? p.defaultFn : null,
                        getHook: p.getHook || null, getHookDirect: !!p.getHookDirect,
                        setHook: p.setHook || null, setHookDirect: !!p.setHookDirect,
                        setHookIsArrow: !!p.setHookIsArrow,
                        attrs: p.attrs || []
                    });
                }
            }
        }
        // Own methods
        if (spec.methods) {
            for (const m of spec.methods) {
                const mk = m.name.toLowerCase();
                const mObj = {
                    name: m.name,
                    vis: m.vis || 0,
                    isStatic: !!m.isStatic,
                    isAbstract: !!m.isAbstract,
                    isFinal: !!m.isFinal,
                    byRef: !!m.byRef,
                    params: m.params || [],
                    retType: m.retType || null,
                    strict: !!m.strict,
                    isGen: !!m.isGen,
                    hasFuncArgs: !!m.hasFuncArgs,
                    direct: !!m.direct,
                    code: m.code || null,
                    builtin: m.builtin || null,
                    refMask: m.refMask || 0,
                    owner: meta,
                    attrs: m.attrs || [],
                    statics: null,
                };
                prepFastMeta(mObj);
                meta.methods.set(mk, mObj);
            }
        }
        // Enum cases + built-in enum methods (cases, from, tryFrom)
        if (meta.kind === "enum") {
            meta.allIfaces.add("unitenum");
            if (meta.backingType) meta.allIfaces.add("backedenum");
            const caseList = [];
            if (spec.cases) {
                for (const ec of spec.cases) {
                    const ep = Object.create(null);
                    ep["name"] = ec.name;
                    const inst = {
                        __php: 2, cls: meta, id: nextObjId++,
                        props: ep,
                        fp: null,
                        readonlyInit: new Set(["name"]),
                        enumCase: ec.name,
                    };
                    if (meta.backingType) {
                        const bv = meta.backingType === "int" ? rt.toInt(ec.val) : rt.toStr(ec.val);
                        ep["value"] = bv;
                        inst.readonlyInit.add("value");
                        inst.enumVal = bv;
                    }
                    meta.enumCases.set(ec.name, inst);
                    meta.consts.set(ec.name, { vis: 0, owner: meta, val: inst, isEnumCase: true });
                    caseList.push(inst);
                }
            }
            meta.methods.set("cases", {
                name: "cases", vis: 0, isStatic: true, owner: meta,
                builtin: function () {
                    const out = rt.makeArr();
                    for (const c of caseList) rt.arrPushInPlace(out, c);
                    return out;
                }
            });
            if (meta.backingType) {
                const findVal = function (val) {
                    val = rt.deref(val);
                    for (const c of caseList) if (rt.strictEq(c.enumVal, val)) return c;
                    return null;
                };
                meta.methods.set("tryfrom", {
                    name: "tryFrom", vis: 0, isStatic: true, owner: meta,
                    builtin: function (val) { return findVal(val); }
                });
                meta.methods.set("from", {
                    name: "from", vis: 0, isStatic: true, owner: meta,
                    builtin: function (val) {
                        const found = findVal(val);
                        if (!found) rt.throwError("ValueError", '"' + rt.toStr(val) + '" is not a valid backing value for enum ' + meta.name);
                        return found;
                    }
                });
            }
        }
        for (const [mk, mv] of meta.methods) meta.mObj[mk] = mv;
        const propInits = [];
        for (const [pk, pm] of meta.props) {
            meta.pObj[pk] = pm;
            if (pm.readonly) meta.hasReadonly = true;
            if (pm.getHook || pm.setHook) meta.hasHooks = true;
            if (pm.hasDefault || (!pm.ty && !pm.getHook)) {
                propInits.push(pm);
            }
        }
        meta.propInits = propInits;
        meta.hasMagicProps = meta.methods.has("__get") || meta.methods.has("__set")
            || meta.methods.has("__isset") || meta.methods.has("__unset");
        meta.hasDestruct = meta.methods.has("__destruct");
        meta.fastProps = meta.kind === "class" && !meta.hasPrivateProps
            && !meta.hasHooks && !meta.hasReadonly && !meta.hasMagicProps;
        classes.set(c, meta);
        return meta;
    }

    R.defClass = makeClassMeta;
    R.defInterface = makeClassMeta;
    R.defTrait = makeClassMeta;
    R.defEnum = makeClassMeta;
    R.resolveCls = function (name) { return resolveClass(name, null, null, false); };
    R.spreadCallArgs = function (posArr, namedObj, src) {
        src = rt.deref(src);
        if (src && src.__php === 1) {
            for (const [k, v] of src.m) {
                if (typeof k === "string") namedObj[k] = rt.deref(v);
                else posArr.push(rt.deref(v));
            }
            return;
        }
        const it = R.iterStart(src, false);
        while (R.iterNext(it)) {
            if (typeof it.key === "string") namedObj[it.key] = it.val;
            else posArr.push(it.val);
        }
    };
    R.calledClassName = function (calledCls) {
        if (!calledCls) rt.throwError("Error", "get_called_class() called from outside a class");
        return calledCls.name;
    };

    // ---- Built-in core classes & Exception hierarchy ----
    const stdClass = makeClassMeta({ name: "stdClass", kind: "class" });
    makeClassMeta({ name: "Attribute", kind: "class" });
    const ClosureClass = makeClassMeta({ name: "Closure", kind: "class", isFinal: true });
    const GeneratorClass = makeClassMeta({ name: "Generator", kind: "class", isFinal: true });
    makeClassMeta({ name: "Traversable", kind: "interface" });
    makeClassMeta({ name: "Iterator", kind: "interface", extends: ["Traversable"] });
    makeClassMeta({ name: "IteratorAggregate", kind: "interface", extends: ["Traversable"] });
    makeClassMeta({ name: "ArrayAccess", kind: "interface" });
    makeClassMeta({ name: "Countable", kind: "interface" });
    makeClassMeta({ name: "Stringable", kind: "interface" });
    makeClassMeta({ name: "JsonSerializable", kind: "interface" });
    makeClassMeta({ name: "UnitEnum", kind: "interface" });
    makeClassMeta({ name: "BackedEnum", kind: "interface", extends: ["UnitEnum"] });
    makeClassMeta({ name: "Throwable", kind: "interface", extends: ["Stringable"] });

    function makeExcClass(name, parentName) {
        return makeClassMeta({
            name: name,
            kind: "class",
            extends: parentName || null,
            implements: parentName ? [] : ["Throwable"],
            props: parentName ? [] : [
                { name: "message", vis: 1, hasDefault: true, defaultVal: "" },
                { name: "code", vis: 1, hasDefault: true, defaultVal: 0n },
                { name: "file", vis: 1, hasDefault: true, defaultVal: "" },
                { name: "line", vis: 1, hasDefault: true, defaultVal: 0n },
                { name: "previous", vis: 2, hasDefault: true, defaultVal: null },
            ],
            methods: parentName ? [] : [
                {
                    name: "__construct", vis: 0,
                    builtin: function (msg, code, prev) {
                        if (msg !== undefined && msg !== null) this.props["message"] = rt.toStr(msg);
                        if (code !== undefined && code !== null) this.props["code"] = rt.toInt(code);
                        if (prev !== undefined) this.props["\0" + name + "\0previous"] = rt.deref(prev);
                    }
                },
                { name: "getMessage", vis: 0, builtin: function () { return this.props["message"] || ""; } },
                { name: "getCode", vis: 0, builtin: function () { return this.props["code"] || 0n; } },
                { name: "getFile", vis: 0, builtin: function () { return this.props["file"] || ""; } },
                { name: "getLine", vis: 0, builtin: function () { return this.props["line"] || 0n; } },
                { name: "getPrevious", vis: 0, builtin: function () { return this.props["\0" + name + "\0previous"] || null; } },
                { name: "getTrace", vis: 0, builtin: function () { return rt.makeArr(); } },
                { name: "getTraceAsString", vis: 0, builtin: function () { return "#0 {main}"; } },
                {
                    name: "__toString", vis: 0,
                    builtin: function () {
                        return this.cls.name + ": " + (this.props["message"] || "");
                    }
                }
            ]
        });
    }
    makeExcClass("Exception", null);
    makeExcClass("RuntimeException", "Exception");
    makeExcClass("LogicException", "Exception");
    makeExcClass("InvalidArgumentException", "LogicException");
    makeExcClass("DomainException", "LogicException");
    makeExcClass("OutOfBoundsException", "RuntimeException");
    makeExcClass("OutOfRangeException", "LogicException");
    makeExcClass("UnexpectedValueException", "RuntimeException");
    makeExcClass("JsonException", "Exception");
    makeExcClass("ErrorException", "Exception");
    makeExcClass("Error", null);
    makeExcClass("TypeError", "Error");
    makeExcClass("ArgumentCountError", "TypeError");
    makeExcClass("ValueError", "Error");
    makeExcClass("ArithmeticError", "Error");
    makeExcClass("DivisionByZeroError", "ArithmeticError");
    makeExcClass("UnhandledMatchError", "Error");
    makeExcClass("AssertionError", "Error");
    makeExcClass("ParseError", "Error");

    function newBuiltinError(clsName, msg) {
        const cls = classes.get(clsName.toLowerCase()) || classes.get("error");
        const obj = allocInstance(cls);
        obj.props["message"] = String(msg);
        try { obj.__stack = (new Error()).stack || ""; } catch (_) {}
        return obj;
    }
    rt.newError = newBuiltinError;
    rt.throwError = function (clsName, msg) {
        throw newBuiltinError(clsName, msg);
    };

    // ---- Object instantiation, cloning, casting ----
    function allocInstance(cls) {
        const p = Object.create(null);
        const inits = cls.propInits;
        for (let i = 0; i < inits.length; i++) {
            const pm = inits[i];
            if (pm.hasDefault) {
                const dv = pm.defaultFn ? pm.defaultFn.call(null, [], null, pm.owner || cls, null) : pm.defaultVal;
                p[pm.key] = (dv !== null && typeof dv === "object") ? rt.share(rt.deref(dv)) : dv;
            } else {
                p[pm.key] = null;
            }
        }
        const obj = {
            __php: 2,
            cls: cls,
            id: nextObjId++,
            props: p,
            fp: cls.fastProps ? p : null,
            readonlyInit: cls.hasReadonly ? new Set() : null,
        };
        if (cls.hasDestruct) {
            destructQueue.push(obj);
        }
        return obj;
    }
    rt.allocInstance = allocInstance;
    rt.runDestructors = function () {
        while (destructQueue.length > 0) {
            const obj = destructQueue.pop();
            const m = findMethod(obj.cls, "__destruct");
            if (m) {
                try { invokeMethod(obj, m, [], null, obj.cls, obj.cls, false); } catch (_) {}
            }
        }
    };

    rt.toObject = function (v) {
        v = rt.deref(v);
        if (v && (v.__php === 2 || v.__php === 3)) return v;
        const obj = allocInstance(stdClass);
        if (v === null || v === undefined) return obj;
        if (v && v.__php === 1) {
            for (const [k, val] of v.m) obj.props[String(k)] = rt.share(rt.deref(val));
            return obj;
        }
        obj.props["scalar"] = v;
        return obj;
    };

    R.newObj = function (classRef, args, named, callerScopeCls, calledCls, callerStrict) {
        const cls = requireClass(classRef, callerScopeCls, calledCls);
        if (cls.isAbstract || cls.kind === "interface" || cls.kind === "trait" || cls.kind === "enum") {
            rt.throwError("Error", "Cannot instantiate " + cls.kind + " " + cls.name);
        }
        const obj = allocInstance(cls);
        const ctor = findMethod(cls, "__construct");
        if (ctor) {
            invokeMethod(obj, ctor, args || [], named || null, callerScopeCls, cls, callerStrict);
        }
        return obj;
    };

    R.cloneObj = function (v) {
        v = rt.deref(v);
        if (!v || v.__php !== 2) rt.throwError("TypeError", "__clone method called on non-object");
        const p = Object.create(null);
        for (const k in v.props) {
            const pv = v.props[k];
            p[k] = (pv !== null && typeof pv === "object") ? rt.share(rt.deref(pv)) : pv;
        }
        const copy = {
            __php: 2, cls: v.cls, id: nextObjId++,
            props: p,
            fp: v.cls.fastProps ? p : null,
            readonlyInit: v.cls.hasReadonly ? new Set() : null,
        };
        const cm = findMethod(v.cls, "__clone");
        if (cm) invokeMethod(copy, cm, [], null, v.cls, v.cls, false);
        return copy;
    };

    R.instanceOf = function (val, classRef, callerScopeCls, calledCls) {
        val = rt.deref(val);
        if (!val || (val.__php !== 2 && val.__php !== 3)) return false;
        const target = resolveClass(classRef, callerScopeCls, calledCls, false);
        if (!target) return false;
        const cls = val.__php === 3 ? ClosureClass : val.cls;
        return isSubtype(cls, target);
    };

    // ---- Property access (with visibility, readonly, magic __get/__set/__isset/__unset) ----
    function findPropDef(cls, name, callerScopeCls) {
        if (!cls.hasPrivateProps) {
            return cls.pObj[name] || null;
        }
        // Check private property on callerScopeCls first
        if (callerScopeCls && callerScopeCls.hasPrivateProps) {
            const pd = cls.pObj["\0" + callerScopeCls.name + "\0" + name];
            if (pd !== undefined) return pd;
        }
        const pub = cls.pObj[name];
        if (pub !== undefined) return pub;
        // Check private property of cls itself (for visibility error message)
        return cls.pObj["\0" + cls.name + "\0" + name] || null;
    }
    function canAccess(vis, owner, callerScopeCls) {
        if (vis === 0) return true;
        if (!callerScopeCls) return false;
        if (vis === 2) return callerScopeCls === owner;
        // protected: same hierarchy
        return isSubtype(callerScopeCls, owner) || isSubtype(owner, callerScopeCls);
    }

    R.propGet = function (obj, name, nullsafe, callerScopeCls, quiet) {
        if (obj === null || obj === undefined) return null;
        if (obj.__php === 5) { obj = obj.v; if (obj === null || obj === undefined) return null; }
        if (obj.__php !== 2) return null;
        if (obj.fp !== null && typeof name === "string") {
            const raw = obj.fp[name];
            if (raw !== undefined) return raw;
        }
        const propName = typeof name === "string" ? name : rt.toStr(name);
        const def = findPropDef(obj.cls, propName, callerScopeCls);
        if (def !== null) {
            if (def.vis !== 0 && !canAccess(def.vis, def.owner, callerScopeCls)) {
                const gm = findMethod(obj.cls, "__get");
                if (gm) return invokeMethod(obj, gm, [propName], null, obj.cls, obj.cls, false);
                rt.throwError("Error", "Cannot access private/protected property " + obj.cls.name + "::$" + propName);
            }
            if (def.getHook && !obj.__inHook) {
                obj.__inHook = true;
                try {
                    return def.getHookDirect
                        ? def.getHook.call(def, null, obj, obj.cls, null)
                        : def.getHook.call(def, [], obj, obj.cls, null);
                } finally {
                    obj.__inHook = false;
                }
            }
            const raw = obj.props[def.key];
            if (raw !== undefined) return (raw !== null && typeof raw === "object" && raw.__php === 5) ? raw.v : raw;
            if (!def.ty) return null;
            const gm = findMethod(obj.cls, "__get");
            if (gm) return invokeMethod(obj, gm, [propName], null, obj.cls, obj.cls, false);
            if (quiet) return null;
            rt.throwError("Error", "Typed property " + obj.cls.name + "::$" + propName + " must not be accessed before initialization");
        }
        const dyn = obj.props[propName];
        if (dyn !== undefined) return (dyn !== null && typeof dyn === "object" && dyn.__php === 5) ? dyn.v : dyn;
        const gm = findMethod(obj.cls, "__get");
        if (gm) return invokeMethod(obj, gm, [propName], null, obj.cls, obj.cls, false);
        return null;
    };

    R.propSet = function (obj, name, val, callerScopeCls) {
        if (obj !== null && typeof obj === "object" && obj.__php === 5) obj = obj.v;
        if (val !== null && typeof val === "object") val = rt.share(val);
        if (!obj || obj.__php !== 2) {
            rt.throwError("Error", "Attempt to assign property on non-object");
        }
        const propName = typeof name === "string" ? name : rt.toStr(name);
        if (obj.fp !== null) {
            const def = obj.cls.pObj[propName];
            if (def !== undefined) {
                const writeVis = def.setVis !== undefined ? def.setVis : def.vis;
                if (writeVis === 0 || canAccess(writeVis, def.owner, callerScopeCls)) {
                    const ty = def.ty;
                    if (ty === null
                        || (ty === "int" && typeof val === "bigint")
                        || (ty === "string" && typeof val === "string")
                        || (ty === "bool" && typeof val === "boolean")
                        || (ty === "array" && val !== null && typeof val === "object" && val.__php === 1)) {
                        obj.fp[propName] = val;
                        return val;
                    }
                    val = checkTypeFast(val, ty, false, def.owner, obj.cls, propName);
                    obj.fp[propName] = val;
                    return val;
                }
            }
        }
        const def = findPropDef(obj.cls, propName, callerScopeCls);
        if (def !== null) {
            const writeVis = def.setVis !== undefined ? def.setVis : def.vis;
            if (writeVis !== 0 && !canAccess(writeVis, def.owner, callerScopeCls)) {
                const sm = findMethod(obj.cls, "__set");
                if (sm) { invokeMethod(obj, sm, [propName, val], null, obj.cls, obj.cls, false); return val; }
                rt.throwError("Error", "Cannot modify private/protected(set) property " + obj.cls.name + "::$" + propName);
            }
            if (def.readonly && obj.readonlyInit && obj.readonlyInit.has(def.key)) {
                rt.throwError("Error", "Cannot modify readonly property " + obj.cls.name + "::$" + propName);
            }
            if (def.ty) val = checkTypeFast(val, def.ty, false, def.owner, obj.cls, propName);
            if (def.setHook && !obj.__inHook) {
                obj.__inHook = true;
                try {
                    const res = def.setHookDirect
                        ? def.setHook.call(def, val, obj, obj.cls, null)
                        : def.setHook.call(def, [val], obj, obj.cls, null);
                    if (def.setHookIsArrow) obj.props[def.key] = res;
                } finally {
                    obj.__inHook = false;
                }
                if (def.readonly && obj.readonlyInit) obj.readonlyInit.add(def.key);
                return val;
            }
            const cur = obj.props[def.key];
            if (cur !== null && cur !== undefined && typeof cur === "object" && cur.__php === 5) cur.v = val;
            else obj.props[def.key] = val;
            if (def.readonly && obj.readonlyInit) obj.readonlyInit.add(def.key);
            return val;
        }
        if (obj.props[propName] === undefined) {
            const sm = findMethod(obj.cls, "__set");
            if (sm) { invokeMethod(obj, sm, [propName, val], null, obj.cls, obj.cls, false); return val; }
        }
        const cur = obj.props[propName];
        if (cur !== null && cur !== undefined && typeof cur === "object" && cur.__php === 5) cur.v = val;
        else obj.props[propName] = val;
        return val;
    };

    R.propIsset = function (obj, name, callerScopeCls) {
        obj = rt.deref(obj);
        if (!obj || obj.__php !== 2) return false;
        const propName = rt.toStr(name);
        const def = findPropDef(obj.cls, propName, callerScopeCls);
        if (def && canAccess(def.vis, def.owner, callerScopeCls) && obj.props[def.key] !== undefined) {
            return rt.deref(obj.props[def.key]) !== null;
        }
        if (!def && obj.props[propName] !== undefined) return rt.deref(obj.props[propName]) !== null;
        const im = findMethod(obj.cls, "__isset");
        if (im) return rt.truthy(invokeMethod(obj, im, [propName], null, obj.cls, obj.cls, false));
        return false;
    };

    R.propUnset = function (obj, name, callerScopeCls) {
        obj = rt.deref(obj);
        if (!obj || obj.__php !== 2) return;
        const propName = rt.toStr(name);
        const def = findPropDef(obj.cls, propName, callerScopeCls);
        if (def && canAccess(def.vis, def.owner, callerScopeCls)) {
            delete obj.props[def.key];
            return;
        }
        if (!def && obj.props[propName] !== undefined) {
            delete obj.props[propName];
            return;
        }
        const um = findMethod(obj.cls, "__unset");
        if (um) invokeMethod(obj, um, [propName], null, obj.cls, obj.cls, false);
    };

    R.propRef = function (obj, name, callerScopeCls) {
        obj = rt.deref(obj);
        if (!obj || obj.__php !== 2) rt.throwError("Error", "Cannot create reference to property of non-object");
        obj.fp = null;
        const propName = rt.toStr(name);
        const def = findPropDef(obj.cls, propName, callerScopeCls);
        const key = (def && canAccess(def.vis, def.owner, callerScopeCls)) ? def.key : propName;
        let cur = obj.props[key];
        if (!rt.isRef(cur)) {
            cur = rt.mkRef(cur === undefined ? null : cur);
            obj.props[key] = cur;
        }
        return cur;
    };

    // ---- Static properties & constants ----
    R.staticGet = function (classRef, prop, callerScopeCls, calledCls) {
        const cls = requireClass(classRef, callerScopeCls, calledCls);
        const sp = cls.staticProps.get(rt.toStr(prop));
        if (!sp) rt.throwError("Error", "Access to undeclared static property " + cls.name + "::$" + prop);
        return sp.ref.v;
    };
    R.staticSet = function (classRef, prop, val, callerScopeCls, calledCls) {
        const cls = requireClass(classRef, callerScopeCls, calledCls);
        const sp = cls.staticProps.get(rt.toStr(prop));
        if (!sp) rt.throwError("Error", "Access to undeclared static property " + cls.name + "::$" + prop);
        val = rt.share(rt.deref(val));
        if (sp.ty) val = checkType(val, sp.ty, false, cls, cls.name + "::$" + prop);
        sp.ref.v = val;
        return val;
    };
    R.staticRef = function (classRef, prop, callerScopeCls, calledCls) {
        const cls = requireClass(classRef, callerScopeCls, calledCls);
        const sp = cls.staticProps.get(rt.toStr(prop));
        if (!sp) rt.throwError("Error", "Access to undeclared static property " + cls.name + "::$" + prop);
        return sp.ref;
    };
    R.staticConst = function (classRef, name, callerScopeCls, calledCls) {
        name = rt.toStr(name);
        if (name.toLowerCase() === "class") {
            if (classRef && typeof classRef === "object" && classRef.__php === 2) return classRef.cls.name;
            const resolved = resolveClass(classRef, callerScopeCls, calledCls, false);
            return resolved ? resolved.name : stripLeading(rt.toStr(classRef));
        }
        const cls = requireClass(classRef, callerScopeCls, calledCls);
        const c = cls.consts.get(name);
        if (!c) rt.throwError("Error", "Undefined constant " + cls.name + "::" + name);
        if (c.resolved === false && typeof c.valFn === "function") {
            c.resolved = true;
            c.val = rt.share(rt.deref(c.valFn.call(null, [], null, c.owner || cls, null)));
        }
        return c.val;
    };

    // ---- Runtime type validation (fast path for common scalar types, lazy error formatting) ----
    function checkTypeFast(val, ty, strict, scopeCls, errOwner, errPart) {
        if (!ty || ty === "mixed") return val;
        // Fast inline checks for the 6 most frequent scalar/array types (avoids toLowerCase/split/string allocs!)
        if (ty === "int") {
            if (typeof val === "bigint") return val;
        } else if (ty === "string") {
            if (typeof val === "string") return val;
        } else if (ty === "bool") {
            if (typeof val === "boolean") return val;
        } else if (ty === "array") {
            if (val !== null && typeof val === "object" && val.__php === 1) return val;
        } else if (ty === "float") {
            if (typeof val === "number") return val;
            if (typeof val === "bigint") return Number(val);
        } else if (ty === "?string") {
            if (val === null || typeof val === "string") return val;
        } else if (ty === "?int") {
            if (val === null || typeof val === "bigint") return val;
        } else if (ty === "?array") {
            if (val === null || (typeof val === "object" && val.__php === 1)) return val;
        } else if (ty === "object") {
            if (val !== null && typeof val === "object" && (val.__php === 2 || val.__php === 3)) return val;
        } else {
            val = rt.deref(val);
            if (matchesType(val, ty, true, scopeCls)) {
                if ((ty === "float" || ty === "?float") && typeof val === "bigint") return Number(val);
                return val;
            }
        }
        val = rt.deref(val);
        if (matchesType(val, ty, true, scopeCls)) {
            if (typeof val === "bigint" && (ty === "float" || ty === "?float")) return Number(val);
            return val;
        }
        if (!strict) {
            const coerced = coerceType(val, ty, scopeCls);
            if (coerced !== undefined) return coerced;
        }
        const label = errOwner ? (typeof errOwner === "string" ? errOwner + errPart : errOwner.name + "::$" + errPart) : String(errPart || "");
        rt.throwError("TypeError", label + ": invalid value of type " + rt.getDebugType(val) + " for type " + ty);
    }
    function checkType(val, ty, strict, scopeCls, label) {
        return checkTypeFast(val, ty, strict, scopeCls, null, label);
    }
    rt.checkType = checkType;

    function matchesType(val, ty, strict, scopeCls) {
        if (ty === "mixed") return true;
        if (ty.startsWith("?")) {
            return val === null || matchesType(val, ty.slice(1), strict, scopeCls);
        }
        if (ty.includes("|")) {
            const parts = ty.split("|");
            for (let i = 0; i < parts.length; i++) {
                if (matchesType(val, parts[i], strict, scopeCls)) return true;
            }
            return false;
        }
        if (ty.includes("&")) {
            const parts = ty.split("&");
            for (let i = 0; i < parts.length; i++) {
                if (!matchesType(val, parts[i], strict, scopeCls)) return false;
            }
            return true;
        }
        const lo = ty.toLowerCase();
        if (lo === "null") return val === null;
        if (lo === "void" || lo === "never") return val === null;
        if (lo === "bool") return typeof val === "boolean";
        if (lo === "true") return val === true;
        if (lo === "false") return val === false;
        if (lo === "int") return typeof val === "bigint";
        if (lo === "float") return typeof val === "number" || typeof val === "bigint";
        if (lo === "string") return typeof val === "string";
        if (lo === "array") return !!(val && val.__php === 1);
        if (lo === "object") return !!(val && (val.__php === 2 || val.__php === 3));
        if (lo === "callable") return isCallable(val);
        if (lo === "iterable") return !!(val && (val.__php === 1 || (val.__php === 2 && isSubtype(val.cls, classes.get("traversable")))));
        const cls = resolveClass(ty, scopeCls, scopeCls, false);
        if (!cls) return false;
        if (val && val.__php === 3) return isSubtype(ClosureClass, cls);
        if (val && val.__php === 2) return isSubtype(val.cls, cls);
        return false;
    }

    function coerceType(val, ty, scopeCls) {
        if (ty.startsWith("?")) {
            if (val === null) return null;
            return coerceType(val, ty.slice(1), scopeCls);
        }
        const parts = ty.includes("|") ? ty.split("|") : [ty];
        for (let i = 0; i < parts.length; i++) {
            const lo = parts[i].toLowerCase();
            if (lo === "int") {
                if (typeof val === "number" || typeof val === "boolean" || (typeof val === "string" && rt.parseNumericString(val))) {
                    return rt.toInt(val);
                }
            } else if (lo === "float") {
                if (typeof val === "bigint" || typeof val === "boolean" || (typeof val === "string" && rt.parseNumericString(val))) {
                    return rt.toFloat(val);
                }
            } else if (lo === "string") {
                if (typeof val === "bigint" || typeof val === "number" || typeof val === "boolean") return rt.toStr(val);
                if (val && val.__php === 2 && findMethod(val.cls, "__tostring")) return rt.toStr(val);
            } else if (lo === "bool") {
                if (typeof val === "bigint" || typeof val === "number" || typeof val === "string") return rt.truthy(val);
            }
        }
        return undefined;
    }

    function isCallable(v) {
        v = rt.deref(v);
        if (!v) return false;
        if (v.__php === 3) return true;
        if (typeof v === "string") {
            if (v.includes("::")) {
                const [c, m] = v.split("::");
                const cls = resolveClass(c, null, null, true);
                return !!(cls && findMethod(cls, m.toLowerCase()));
            }
            return rt.funcs.has(canon(v));
        }
        if (v.__php === 1 && v.m.size === 2) {
            const vals = Array.from(v.m.values()).map(rt.deref);
            if (typeof vals[1] !== "string") return false;
            if (vals[0] && vals[0].__php === 2) return !!(findMethod(vals[0].cls, vals[1].toLowerCase()) || findMethod(vals[0].cls, "__call"));
            if (typeof vals[0] === "string") {
                const cls = resolveClass(vals[0], null, null, true);
                return !!(cls && (findMethod(cls, vals[1].toLowerCase()) || findMethod(cls, "__callstatic")));
            }
        }
        if (v.__php === 2 && findMethod(v.cls, "__invoke")) return true;
        return false;
    }
    rt.isCallable = isCallable;

    // ---- Function / Method / Closure invocation ----
    function findMethod(cls, lowerName) {
        return cls ? (cls.methods.get(lowerName) || null) : null;
    }
    function findMethodForCall(targetCls, lowerName, callerScopeCls) {
        if (callerScopeCls && targetCls && isSubtype(targetCls, callerScopeCls)) {
            const pm = callerScopeCls.methods.get(lowerName);
            if (pm && pm.vis === 2 && pm.owner === callerScopeCls) return pm;
        }
        return targetCls ? (targetCls.methods.get(lowerName) || null) : null;
    }
    rt.findMethod = findMethod;

    function bindUserArgs(fnMeta, posArgs, namedArgs, callerStrict) {
        const params = fnMeta.params;
        const pLen = params ? params.length : 0;
        const posLen = posArgs.length;
        // Ultra-fast path: no named args, no variadic, positional count <= params count
        if (namedArgs === null) {
            if (pLen === 0) {
                if (fnMeta.hasFuncArgs) { const b = []; b._raw = posArgs; return b; }
                return [];
            }
            const hasVariadic = params[pLen - 1].variadic;
            const fixedCount = hasVariadic ? pLen - 1 : pLen;
            const bound = new Array(pLen);
            const scopeCls = fnMeta.owner || fnMeta.scopeCls;
            const passN = posLen < fixedCount ? posLen : fixedCount;
            for (let i = 0; i < passN; i++) {
                const p = params[i];
                const raw = posArgs[i];
                if (p.byRef) {
                    bound[i] = rt.isRef(raw) ? raw : rt.mkRef(raw);
                } else {
                    let v = rt.deref(raw);
                    if (v !== null && typeof v === "object" && v.__php === 1) v.shared = true;
                    if (p.ty) v = checkTypeFast(v, p.ty, callerStrict, scopeCls, fnMeta.name, "(): Argument #" + (i + 1) + " ($" + p.name + ")");
                    bound[i] = v;
                }
            }
            for (let i = passN; i < fixedCount; i++) {
                const p = params[i];
                if (p.hasDefault) {
                    const dv = typeof p.defaultFn === "function" ? p.defaultFn.call(null, [], null, scopeCls, null) : p.defaultVal;
                    bound[i] = p.byRef ? rt.mkRef(dv) : rt.share(rt.deref(dv));
                } else {
                    rt.throwError("ArgumentCountError", "Too few arguments to function " + fnMeta.name + "(), " + posLen + " passed and at least " + (i + 1) + " expected");
                }
            }
            if (hasVariadic) {
                const variadicArr = rt.makeArr();
                const vp = params[fixedCount];
                for (let i = fixedCount; i < posLen; i++) {
                    let v = rt.deref(posArgs[i]);
                    if (vp.ty) v = checkTypeFast(v, vp.ty, callerStrict, scopeCls, fnMeta.name, "()");
                    rt.arrPushInPlace(variadicArr, v);
                }
                bound[fixedCount] = variadicArr;
            }
            if (fnMeta.hasFuncArgs) bound._raw = posArgs;
            return bound;
        }

        const bound = new Array(pLen);
        const filled = new Array(pLen).fill(false);
        const hasVariadic = pLen > 0 && params[pLen - 1].variadic;
        const fixedCount = hasVariadic ? pLen - 1 : pLen;
        const variadicArr = hasVariadic ? rt.makeArr() : null;
        const scopeCls = fnMeta.owner || fnMeta.scopeCls;

        for (let i = 0; i < posLen; i++) {
            const raw = posArgs[i];
            if (i < fixedCount) {
                const p = params[i];
                let v = p.byRef ? (rt.isRef(raw) ? raw : rt.mkRef(raw)) : rt.share(rt.deref(raw));
                if (p.ty && !p.byRef) v = checkTypeFast(v, p.ty, callerStrict, scopeCls, fnMeta.name, "(): Argument #" + (i + 1) + " ($" + p.name + ")");
                bound[i] = v;
                filled[i] = true;
            } else if (hasVariadic) {
                const vp = params[fixedCount];
                let v = rt.deref(raw);
                if (vp.ty) v = checkTypeFast(v, vp.ty, callerStrict, scopeCls, fnMeta.name, "()");
                rt.arrPushInPlace(variadicArr, v);
            }
        }
        for (const [k, raw] of Object.entries(namedArgs)) {
            const idx = params.findIndex(p => p.name === k);
            if (idx >= 0 && idx < fixedCount) {
                const p = params[idx];
                let v = p.byRef ? (rt.isRef(raw) ? raw : rt.mkRef(raw)) : rt.share(rt.deref(raw));
                if (p.ty && !p.byRef) v = checkTypeFast(v, p.ty, callerStrict, scopeCls, fnMeta.name, "(): Argument $" + p.name);
                bound[idx] = v;
                filled[idx] = true;
            } else if (hasVariadic) {
                rt.arrSetInPlace(variadicArr, k, rt.deref(raw));
            } else {
                rt.throwError("Error", "Unknown named parameter $" + k + " in call to " + fnMeta.name + "()");
            }
        }
        for (let i = 0; i < fixedCount; i++) {
            if (!filled[i]) {
                const p = params[i];
                if (p.hasDefault) {
                    const dv = typeof p.defaultFn === "function" ? p.defaultFn.call(null, [], null, scopeCls, null) : p.defaultVal;
                    bound[i] = p.byRef ? rt.mkRef(dv) : rt.share(rt.deref(dv));
                } else {
                    rt.throwError("ArgumentCountError", "Too few arguments to function " + fnMeta.name + "(), " + posLen + " passed and at least " + (i + 1) + " expected");
                }
            }
        }
        if (hasVariadic) bound[fixedCount] = variadicArr;
        if (fnMeta.hasFuncArgs) bound._raw = posArgs;
        return bound;
    }

    function invokeFnObj(fn, args, named, phpThis, calledCls, callerStrict) {
        if (fn.builtin) {
            const mask = fn.refMask || 0;
            if (mask === 0) {
                let needDeref = false;
                for (let i = 0; i < args.length; i++) {
                    const a = args[i];
                    if (a !== null && typeof a === "object" && a.__php === 5) { needDeref = true; break; }
                }
                const res = needDeref
                    ? fn.builtin.apply(phpThis, args.map(rt.deref))
                    : fn.builtin.apply(phpThis, args);
                return res === undefined ? null : res;
            }
            const finalArgs = new Array(args.length);
            for (let i = 0; i < args.length; i++) {
                const needRef = (mask & (1 << i)) !== 0;
                finalArgs[i] = needRef ? (rt.isRef(args[i]) ? args[i] : rt.mkRef(args[i])) : rt.deref(args[i]);
            }
            const res = fn.builtin.apply(phpThis, finalArgs);
            return res === undefined ? null : res;
        }
        const bound = bindUserArgs(fn, args, named, callerStrict);
        const res = fn.direct
            ? fn.code(bound[0], phpThis, calledCls, bound[1])
            : fn.code(bound, phpThis, calledCls, null);
        if (fn.isGen) return makePhpGenerator(res);
        const rtTy = fn.retType;
        if (rtTy && rtTy !== "void") {
            const checkCls = (rtTy === "static" || rtTy === "?static") ? (calledCls || fn.owner || fn.scopeCls) : (fn.owner || fn.scopeCls);
            return checkTypeFast(res, rtTy, fn.strict, checkCls, fn.name, "(): Return value");
        }
        return res === undefined ? null : res;
    }

    function invokeMethod(obj, method, args, named, callerScopeCls, calledCls, callerStrict) {
        if (!canAccess(method.vis, method.owner, callerScopeCls)) {
            rt.throwError("Error", "Call to private/protected method " + method.owner.name + "::" + method.name + "()");
        }
        return invokeFnObj(method, args || [], named || null, method.isStatic ? null : obj, calledCls || (obj ? obj.cls : method.owner), callerStrict);
    }
    rt.invokeMethod = invokeMethod;

    rt.curScopeCls = null;

    function callAny(callee, args, named, callerScopeCls, callerStrict) {
        callee = rt.deref(callee);
        args = args || [];
        const scope = callerScopeCls || rt.curScopeCls || null;
        if (callee && callee.__php === 3) {
            return invokeFnObj(callee, args, named, callee.boundThis || null, callee.calledCls || callee.scopeCls || null, callerStrict);
        }
        if (typeof callee === "string") {
            if (callee.includes("::")) {
                const idx = callee.indexOf("::");
                return R.staticCall(callee.slice(0, idx), callee.slice(idx + 2), args, named, null, scope, scope, callerStrict);
            }
            const fn = rt.funcs.get(canon(callee));
            if (!fn) rt.throwError("Error", "Call to undefined function " + callee + "()");
            return invokeFnObj(fn, args, named, null, null, callerStrict);
        }
        if (callee && callee.__php === 1 && callee.m.size === 2) {
            const pair = Array.from(callee.m.values()).map(rt.deref);
            if (pair[0] && pair[0].__php === 2) {
                return R.methodCall(pair[0], pair[1], false, args, named, scope, callerStrict);
            }
            return R.staticCall(pair[0], pair[1], args, named, null, scope, scope, callerStrict);
        }
        if (callee && callee.__php === 2) {
            const inv = findMethod(callee.cls, "__invoke");
            if (inv) return invokeMethod(callee, inv, args, named, scope, callee.cls, callerStrict);
        }
        rt.throwError("Error", "Value of type " + rt.getDebugType(callee) + " is not callable");
    }
    rt.call = callAny;
    R.call = callAny;

    R.callName = function (fqn, fallback, args, named, callerScopeCls, callerStrict) {
        let fn = rt.funcs.get(canon(fqn));
        if (!fn && fallback !== null) fn = rt.funcs.get(canon(fallback));
        if (!fn) rt.throwError("Error", "Call to undefined function " + fqn + "()");
        if (fn.builtin && callerScopeCls) {
            const prev = rt.curScopeCls;
            rt.curScopeCls = callerScopeCls;
            try { return invokeFnObj(fn, args || [], named || null, null, null, callerStrict); }
            finally { rt.curScopeCls = prev; }
        }
        return invokeFnObj(fn, args || [], named || null, null, null, callerStrict);
    };

    R.methodCall = function (obj, name, nullsafe, args, named, callerScopeCls, callerStrict) {
        obj = rt.deref(obj);
        if (obj === null || obj === undefined) {
            if (nullsafe) return null;
            rt.throwError("Error", "Call to a member function " + name + "() on null");
        }
        const mname = rt.toStr(name);
        if (obj.__php === 3) {
            // Closure methods: bindTo, call
            const lo = mname.toLowerCase();
            if (lo === "call") {
                const newThis = rt.deref(args[0]);
                const rest = args.slice(1);
                const newScope = newThis && newThis.__php === 2 ? newThis.cls : obj.scopeCls;
                const rebound = newScope !== obj.scopeCls ? Object.assign({}, obj, { scopeCls: newScope }) : obj;
                return invokeFnObj(rebound, rest, named, newThis, newScope, callerStrict);
            }
            if (lo === "bindto") {
                const newThis = rt.deref(args[0]);
                let newScope = newThis && newThis.__php === 2 ? newThis.cls : obj.scopeCls;
                if (args.length > 1) {
                    const sc = rt.deref(args[1]);
                    if (sc === null) newScope = null;
                    else if (typeof sc === "string" && sc.toLowerCase() === "static") newScope = obj.scopeCls;
                    else newScope = resolveClass(sc, callerScopeCls, callerScopeCls, true);
                }
                return Object.assign({}, obj, { boundThis: newThis, scopeCls: newScope });
            }
        }
        if (obj.__php !== 2) {
            rt.throwError("Error", "Call to a member function " + mname + "() on " + rt.getDebugType(obj));
        }
        const m = findMethodForCall(obj.cls, mname.toLowerCase(), callerScopeCls);
        if (m) return invokeMethod(obj, m, args || [], named || null, callerScopeCls, obj.cls, callerStrict);
        const magic = findMethod(obj.cls, "__call");
        if (magic) {
            const arr = rt.makeArr();
            for (const a of (args || [])) rt.arrPushInPlace(arr, a);
            return invokeMethod(obj, magic, [mname, arr], null, obj.cls, obj.cls, false);
        }
        rt.throwError("Error", "Call to undefined method " + obj.cls.name + "::" + mname + "()");
    };

    R.staticCall = function (classRef, name, args, named, phpThis, callerScopeCls, calledCls, callerStrict) {
        const cls = requireClass(classRef, callerScopeCls, calledCls);
        const mname = rt.toStr(name);
        const lsb = (typeof classRef === "string" && (classRef.toLowerCase() === "self" || classRef.toLowerCase() === "parent" || classRef.toLowerCase() === "static"))
            ? (calledCls || cls)
            : cls;
        const m = findMethodForCall(cls, mname.toLowerCase(), callerScopeCls);
        if (m) {
            const targetThis = (!m.isStatic && phpThis && isSubtype(phpThis.cls, cls)) ? phpThis : null;
            return invokeMethod(targetThis, m, args || [], named || null, callerScopeCls, lsb, callerStrict);
        }
        const magic = findMethod(cls, "__callstatic");
        if (magic) {
            const arr = rt.makeArr();
            for (const a of (args || [])) rt.arrPushInPlace(arr, a);
            return invokeMethod(null, magic, [mname, arr], null, cls, lsb, false);
        }
        rt.throwError("Error", "Call to undefined method " + cls.name + "::" + mname + "()");
    };

    function tyTag(ty) {
        if (ty === null || ty === undefined || ty === "void") return 0;
        if (ty === "int") return 1;
        if (ty === "string") return 2;
        if (ty === "bool") return 3;
        if (ty === "array") return 4;
        return -1;
    }

    function prepFastMeta(fn) {
        if (fn.builtin || fn.isGen || fn.hasFuncArgs || !fn.code) {
            fn.fastN = -1;
            return;
        }
        const ps = fn.params;
        const n = ps ? ps.length : 0;
        if (n > 2) { fn.fastN = -1; return; }
        for (let i = 0; i < n; i++) {
            if (ps[i].byRef || ps[i].variadic) { fn.fastN = -1; return; }
        }
        fn.fastN = n;
        fn.p0Ty = n >= 1 ? (ps[0].ty || null) : null;
        fn.p1Ty = n >= 2 ? (ps[1].ty || null) : null;
        fn.p0Tag = tyTag(fn.p0Ty);
        fn.p1Tag = tyTag(fn.p1Ty);
        fn.retTag = tyTag(fn.retType);
        if (fn.fastInt) {
            if (n === 1) fn.fastInt1 = fn.fastInt;
            else if (n === 2) fn.fastInt2 = fn.fastInt;
        }
        if (fn.vis === 0 && !fn.isStatic && fn.direct) {
            if (n === 0 && fn.retTag === 0) fn.f0 = fn.code;
            else if (n === 1 && fn.p0Tag === 1 && fn.retTag === 1) fn.fi1 = fn.code;
        }
    }

    function fastArgVal(v, ty, strict, scopeCls, fnName, idx) {
        if (v !== null && typeof v === "object") {
            if (v.__php === 5) v = v.v;
            if (v !== null && typeof v === "object" && v.__php === 1) v.shared = true;
        }
        if (ty !== null) {
            if (ty === "int") { if (typeof v === "bigint") return v; }
            else if (ty === "string") { if (typeof v === "string") return v; }
            else if (ty === "bool") { if (typeof v === "boolean") return v; }
            else if (ty === "array") { if (v !== null && typeof v === "object" && v.__php === 1) return v; }
            return checkTypeFast(v, ty, strict, scopeCls, fnName, "(): Argument #" + idx);
        }
        return v;
    }

    function fastRetVal(res, fn, calledCls) {
        const ty = fn.retType;
        if (ty === null || ty === "void") return res === undefined ? null : res;
        if (ty === "int") { if (typeof res === "bigint") return res; }
        else if (ty === "string") { if (typeof res === "string") return res; }
        else if (ty === "bool") { if (typeof res === "boolean") return res; }
        else if (ty === "array") { if (res !== null && typeof res === "object" && res.__php === 1) return res; }
        const checkCls = (ty === "static" || ty === "?static") ? (calledCls || fn.owner || fn.scopeCls) : (fn.owner || fn.scopeCls);
        return checkTypeFast(res, ty, fn.strict, checkCls, fn.name, "(): Return value");
    }
    R.checkRet = fastRetVal;

    R.callName0 = function (loFqn, loFb, callerScopeCls, callerStrict) {
        let fn = rt.funcs.get(loFqn);
        if (fn === undefined && loFb !== null) fn = rt.funcs.get(loFb);
        if (fn === undefined) rt.throwError("Error", "Call to undefined function " + loFqn + "()");
        if (fn.fastN === 0) {
            const r = fn.direct ? fn.code(null, null, null, null) : fn.code([], null, null, null);
            const rt0 = fn.retTag;
            if (rt0 === 0) return r === undefined ? null : r;
            if ((rt0 === 1 && typeof r === "bigint") || (rt0 === 2 && typeof r === "string") || (rt0 === 3 && typeof r === "boolean")) return r;
            return fastRetVal(r, fn, null);
        }
        if (fn.builtin) {
            const r = fn.builtin();
            return r === undefined ? null : r;
        }
        return invokeFnObj(fn, [], null, null, null, callerStrict);
    };

    R.callName1 = function (loFqn, loFb, a0, callerScopeCls, callerStrict) {
        let fn = rt.funcs.get(loFqn);
        if (fn === undefined && loFb !== null) fn = rt.funcs.get(loFb);
        if (fn === undefined) rt.throwError("Error", "Call to undefined function " + loFqn + "()");
        const fi = fn.fastInt1;
        if (fi !== undefined && typeof a0 === "bigint") {
            const b = fn.fiBound;
            if (a0 >= -b && a0 <= b) {
                const r = fi(Number(a0));
                if (r !== undefined) return BigInt(r);
            }
        }
        if (fn.fastN === 1) {
            let v0 = a0;
            const t0 = fn.p0Tag;
            if (t0 === 1 && typeof v0 === "bigint") {
                // fast int arg
            } else if (t0 === 2 && typeof v0 === "string") {
                // fast string arg
            } else if (t0 === 3 && typeof v0 === "boolean") {
                // fast bool arg
            } else if (t0 === 0 && (v0 === null || typeof v0 !== "object")) {
                // fast untyped primitive
            } else {
                v0 = fastArgVal(a0, fn.p0Ty, callerStrict, null, fn.name, 1);
            }
            const r = fn.direct ? fn.code(v0, null, null, null) : fn.code([v0], null, null, null);
            const rt0 = fn.retTag;
            if (rt0 === 0) return r === undefined ? null : r;
            if ((rt0 === 1 && typeof r === "bigint") || (rt0 === 2 && typeof r === "string") || (rt0 === 3 && typeof r === "boolean")) return r;
            return fastRetVal(r, fn, null);
        }
        if (fn.builtin && !fn.refMask) {
            if (a0 !== null && typeof a0 === "object" && a0.__php === 5) a0 = a0.v;
            if (callerScopeCls) {
                const prev = rt.curScopeCls;
                rt.curScopeCls = callerScopeCls;
                try { const r = fn.builtin(a0); return r === undefined ? null : r; }
                finally { rt.curScopeCls = prev; }
            }
            const r = fn.builtin(a0);
            return r === undefined ? null : r;
        }
        if (fn.builtin && callerScopeCls) {
            const prev = rt.curScopeCls;
            rt.curScopeCls = callerScopeCls;
            try { return invokeFnObj(fn, [a0], null, null, null, callerStrict); }
            finally { rt.curScopeCls = prev; }
        }
        return invokeFnObj(fn, [a0], null, null, null, callerStrict);
    };

    R.callName2 = function (loFqn, loFb, a0, a1, callerScopeCls, callerStrict) {
        let fn = rt.funcs.get(loFqn);
        if (fn === undefined && loFb !== null) fn = rt.funcs.get(loFb);
        if (fn === undefined) rt.throwError("Error", "Call to undefined function " + loFqn + "()");
        const fi = fn.fastInt2;
        if (fi !== undefined && typeof a0 === "bigint" && typeof a1 === "bigint") {
            const b = fn.fiBound;
            if (a0 >= -b && a0 <= b && a1 >= -b && a1 <= b) {
                const r = fi(Number(a0), Number(a1));
                if (r !== undefined) return BigInt(r);
            }
        }
        if (fn.fastN === 2) {
            const v0 = (fn.p0Tag === 1 && typeof a0 === "bigint") || (fn.p0Tag === 2 && typeof a0 === "string")
                ? a0 : fastArgVal(a0, fn.p0Ty, callerStrict, null, fn.name, 1);
            const v1 = (fn.p1Tag === 1 && typeof a1 === "bigint") || (fn.p1Tag === 2 && typeof a1 === "string")
                ? a1 : fastArgVal(a1, fn.p1Ty, callerStrict, null, fn.name, 2);
            const r = fn.direct ? fn.code(v0, null, null, v1) : fn.code([v0, v1], null, null, null);
            const rt0 = fn.retTag;
            if (rt0 === 0) return r === undefined ? null : r;
            if ((rt0 === 1 && typeof r === "bigint") || (rt0 === 2 && typeof r === "string") || (rt0 === 3 && typeof r === "boolean")) return r;
            return fastRetVal(r, fn, null);
        }
        if (fn.builtin && !fn.refMask) {
            if (a0 !== null && typeof a0 === "object" && a0.__php === 5) a0 = a0.v;
            if (a1 !== null && typeof a1 === "object" && a1.__php === 5) a1 = a1.v;
            if (callerScopeCls) {
                const prev = rt.curScopeCls;
                rt.curScopeCls = callerScopeCls;
                try { const r = fn.builtin(a0, a1); return r === undefined ? null : r; }
                finally { rt.curScopeCls = prev; }
            }
            const r = fn.builtin(a0, a1);
            return r === undefined ? null : r;
        }
        if (fn.builtin && callerScopeCls) {
            const prev = rt.curScopeCls;
            rt.curScopeCls = callerScopeCls;
            try { return invokeFnObj(fn, [a0, a1], null, null, null, callerStrict); }
            finally { rt.curScopeCls = prev; }
        }
        return invokeFnObj(fn, [a0, a1], null, null, null, callerStrict);
    };

    R.mCall0 = function (obj, loName, callerScopeCls, callerStrict) {
        if (obj !== null && typeof obj === "object" && obj.__php === 2) {
            const cls = obj.cls;
            let m = cls.mObj[loName];
            if (m === undefined) { m = cls.methods.get(loName); if (m !== undefined) cls.mObj[loName] = m; }
            if (callerScopeCls && callerScopeCls !== cls && isSubtype(cls, callerScopeCls)) {
                const pm = callerScopeCls.mObj[loName];
                if (pm !== undefined && pm.vis === 2 && pm.owner === callerScopeCls) m = pm;
            }
            if (m !== undefined && m.fastN === 0 && (m.vis === 0 || canAccess(m.vis, m.owner, callerScopeCls))) {
                const th = m.isStatic ? null : obj;
                const r = m.direct ? m.code(null, th, cls, null) : m.code([], th, cls, null);
                const rt0 = m.retTag;
                if (rt0 === 0) return r === undefined ? null : r;
                if ((rt0 === 1 && typeof r === "bigint") || (rt0 === 2 && typeof r === "string") || (rt0 === 3 && typeof r === "boolean")) return r;
                return fastRetVal(r, m, cls);
            }
        }
        return R.methodCall(obj, loName, false, [], null, callerScopeCls, callerStrict);
    };

    R.mCall1 = function (obj, loName, a0, callerScopeCls, callerStrict) {
        if (obj !== null && typeof obj === "object" && obj.__php === 2) {
            const cls = obj.cls;
            let m = cls.mObj[loName];
            if (m === undefined) { m = cls.methods.get(loName); if (m !== undefined) cls.mObj[loName] = m; }
            if (callerScopeCls && callerScopeCls !== cls && isSubtype(cls, callerScopeCls)) {
                const pm = callerScopeCls.mObj[loName];
                if (pm !== undefined && pm.vis === 2 && pm.owner === callerScopeCls) m = pm;
            }
            if (m !== undefined && m.fastN === 1 && (m.vis === 0 || canAccess(m.vis, m.owner, callerScopeCls))) {
                let v0 = a0;
                if (v0 !== null && typeof v0 === "object" && v0.__php === 5) v0 = v0.v;
                const t0 = m.p0Tag;
                if (t0 === 1 && typeof v0 === "bigint") {
                    // fast int arg
                } else if (t0 === 2 && typeof v0 === "string") {
                    // fast string arg
                } else if (t0 === 3 && typeof v0 === "boolean") {
                    // fast bool arg
                } else if (t0 === 0 && (v0 === null || typeof v0 !== "object")) {
                    // fast untyped primitive
                } else {
                    v0 = fastArgVal(v0, m.p0Ty, callerStrict, m.owner, m.name, 1);
                }
                const th = m.isStatic ? null : obj;
                const r = m.direct ? m.code(v0, th, cls, null) : m.code([v0], th, cls, null);
                const rt0 = m.retTag;
                if (rt0 === 0) return r === undefined ? null : r;
                if ((rt0 === 1 && typeof r === "bigint") || (rt0 === 2 && typeof r === "string") || (rt0 === 3 && typeof r === "boolean")) return r;
                return fastRetVal(r, m, cls);
            }
        }
        return R.methodCall(obj, loName, false, [a0], null, callerScopeCls, callerStrict);
    };

    R.mCall2 = function (obj, loName, a0, a1, callerScopeCls, callerStrict) {
        if (obj !== null && typeof obj === "object" && obj.__php === 2) {
            const cls = obj.cls;
            let m = cls.mObj[loName];
            if (m === undefined) { m = cls.methods.get(loName); if (m !== undefined) cls.mObj[loName] = m; }
            if (callerScopeCls && callerScopeCls !== cls && isSubtype(cls, callerScopeCls)) {
                const pm = callerScopeCls.mObj[loName];
                if (pm !== undefined && pm.vis === 2 && pm.owner === callerScopeCls) m = pm;
            }
            if (m !== undefined && m.fastN === 2 && (m.vis === 0 || canAccess(m.vis, m.owner, callerScopeCls))) {
                const v0 = (m.p0Tag === 1 && typeof a0 === "bigint") || (m.p0Tag === 2 && typeof a0 === "string")
                    ? a0 : fastArgVal(a0, m.p0Ty, callerStrict, m.owner, m.name, 1);
                const v1 = (m.p1Tag === 1 && typeof a1 === "bigint") || (m.p1Tag === 2 && typeof a1 === "string")
                    ? a1 : fastArgVal(a1, m.p1Ty, callerStrict, m.owner, m.name, 2);
                const th = m.isStatic ? null : obj;
                const r = m.direct ? m.code(v0, th, cls, v1) : m.code([v0, v1], th, cls, null);
                const rt0 = m.retTag;
                if (rt0 === 0) return r === undefined ? null : r;
                if ((rt0 === 1 && typeof r === "bigint") || (rt0 === 2 && typeof r === "string") || (rt0 === 3 && typeof r === "boolean")) return r;
                return fastRetVal(r, m, cls);
            }
        }
        return R.methodCall(obj, loName, false, [a0, a1], null, callerScopeCls, callerStrict);
    };

    R.defFn = function (fqn, code, meta) {
        const fn = Object.assign({
            __php: 3,
            name: stripLeading(fqn),
            code: code,
            params: [],
            byRef: false,
            retType: null,
            strict: false,
            isGen: false,
            statics: null,
        }, meta || {});
        prepFastMeta(fn);
        rt.funcs.set(canon(fqn), fn);
    };

    R.mkClosure = function (code, meta, uses, phpThis, scopeCls) {
        return Object.assign({
            __php: 3,
            name: "{closure}",
            code: code,
            params: [],
            byRef: false,
            retType: null,
            strict: false,
            isGen: false,
            uses: uses || [],
            boundThis: meta && meta.isStatic ? null : (phpThis || null),
            scopeCls: scopeCls || null,
            statics: null,
        }, meta || {});
    };

    function wrapResolvedCallable(fn, boundThis, scopeCls, calledCls) {
        if (fn && !fn.builtin) {
            return Object.assign({}, fn, {
                __php: 3,
                boundThis: boundThis || null,
                scopeCls: scopeCls || fn.owner || fn.scopeCls || null,
                calledCls: calledCls || scopeCls || fn.owner || null,
            });
        }
        return {
            __php: 3,
            name: fn ? fn.name : "{closure}",
            params: fn && fn.params ? fn.params : [],
            retType: fn ? fn.retType : null,
            builtin: fn && fn.builtin ? fn.builtin : null,
            refMask: fn ? (fn.refMask || 0) : 0,
            boundThis: boundThis || null,
            scopeCls: scopeCls || null,
            owner: fn ? fn.owner : null,
        };
    }

    R.firstClass = function (target, method, isStatic, calledCls, callerScopeCls) {
        target = rt.deref(target);
        if (method === null) {
            if (target && target.__php === 3) return target;
            if (target && target.__php === 1 && target.m.size === 2) {
                const pair = Array.from(target.m.values()).map(rt.deref);
                if (pair[0] && pair[0].__php === 2) {
                    return R.firstClass(pair[0], rt.toStr(pair[1]), false, pair[0].cls, callerScopeCls);
                }
                return R.firstClass(pair[0], rt.toStr(pair[1]), true, calledCls, callerScopeCls);
            }
            if (target && target.__php === 2) {
                const inv = findMethod(target.cls, "__invoke");
                if (inv) return wrapResolvedCallable(inv, target, target.cls, target.cls);
            }
            const name = rt.toStr(target);
            if (name.includes("::")) {
                const idx = name.indexOf("::");
                return R.firstClass(name.slice(0, idx), name.slice(idx + 2), true, calledCls, callerScopeCls);
            }
            const fn = rt.funcs.get(canon(name));
            if (fn) return wrapResolvedCallable(fn, null, null, null);
            return {
                __php: 3, name: name, params: [],
                builtin: function () { return callAny(name, Array.from(arguments), null, callerScopeCls, false); }
            };
        }
        const mname = rt.toStr(method);
        if (isStatic) {
            const cls = requireClass(target, callerScopeCls, calledCls);
            const m = findMethodForCall(cls, mname.toLowerCase(), callerScopeCls);
            if (m) {
                if (!canAccess(m.vis, m.owner, callerScopeCls)) {
                    rt.throwError("Error", "Call to private/protected method " + m.owner.name + "::" + m.name + "()");
                }
                return wrapResolvedCallable(m, null, m.owner, cls);
            }
            return {
                __php: 3, name: cls.name + "::" + mname, params: [],
                builtin: function () { return R.staticCall(cls, mname, Array.from(arguments), null, null, callerScopeCls, cls, false); }
            };
        }
        const obj = target;
        if (obj && obj.__php === 2) {
            const m = findMethodForCall(obj.cls, mname.toLowerCase(), callerScopeCls);
            if (m) {
                if (!canAccess(m.vis, m.owner, callerScopeCls)) {
                    rt.throwError("Error", "Call to private/protected method " + m.owner.name + "::" + m.name + "()");
                }
                return wrapResolvedCallable(m, m.isStatic ? null : obj, m.owner, obj.cls);
            }
        }
        return {
            __php: 3, name: mname, params: [],
            builtin: function () { return R.methodCall(obj, mname, false, Array.from(arguments), null, callerScopeCls, false); }
        };
    };

    // ---- PHP Generators backed by ZIPP's native JS generators ----
    function makePhpGenerator(jsGen) {
        const state = { gen: jsGen, started: false, done: false, key: null, val: null, ret: null, autoKey: 0n };
        const obj = allocInstance(GeneratorClass);
        obj.__gen = state;
        return obj;
    }
    function genEnsureStarted(g) {
        const st = g.__gen;
        if (st.started) return;
        st.started = true;
        genStep(st, undefined);
    }
    function genAdvance(g, sendVal) {
        const st = g.__gen;
        if (!st.started) { st.started = true; genStep(st, undefined); return; }
        if (st.done) return;
        genStep(st, sendVal);
    }
    function genStep(st, sendVal) {
        const r = st.gen.next(sendVal);
        if (r.done) {
            st.done = true;
            st.key = null;
            st.val = null;
            st.ret = r.value === undefined ? null : r.value;
        } else {
            const pair = r.value; // [hasKey, key, val]
            if (pair && pair.__yield) {
                if (pair.hasKey) {
                    st.key = pair.key;
                    if (typeof pair.key === "bigint" && pair.key >= st.autoKey) st.autoKey = pair.key + 1n;
                } else {
                    st.key = st.autoKey++;
                }
                st.val = pair.val;
            } else {
                st.key = st.autoKey++;
                st.val = pair;
            }
        }
    }
    rt.genEnsureStarted = genEnsureStarted;
    rt.genAdvance = genAdvance;
    R.yieldVal = function (hasKey, key, val) {
        return { __yield: true, hasKey: !!hasKey, key: hasKey ? rt.deref(key) : null, val: rt.share(rt.deref(val)) };
    };
    GeneratorClass.methods.set("valid", { name: "valid", vis: 0, owner: GeneratorClass, builtin: function () { genEnsureStarted(this); return !this.__gen.done; } });
    GeneratorClass.methods.set("current", { name: "current", vis: 0, owner: GeneratorClass, builtin: function () { genEnsureStarted(this); return this.__gen.val; } });
    GeneratorClass.methods.set("key", { name: "key", vis: 0, owner: GeneratorClass, builtin: function () { genEnsureStarted(this); return this.__gen.key; } });
    GeneratorClass.methods.set("next", { name: "next", vis: 0, owner: GeneratorClass, builtin: function () { genAdvance(this, null); } });
    GeneratorClass.methods.set("rewind", { name: "rewind", vis: 0, owner: GeneratorClass, builtin: function () { genEnsureStarted(this); } });
    GeneratorClass.methods.set("send", { name: "send", vis: 0, owner: GeneratorClass, builtin: function (v) { genEnsureStarted(this); genAdvance(this, rt.deref(v)); return this.__gen.val; } });
    GeneratorClass.methods.set("getreturn", { name: "getReturn", vis: 0, owner: GeneratorClass, builtin: function () { return this.__gen.ret; } });

    // ---- Exceptions & match ----
    R.throwVal = function (v) {
        v = rt.deref(v);
        if (!v || v.__php !== 2 || !isSubtype(v.cls, classes.get("throwable"))) {
            rt.throwError("Error", "Can only throw objects implementing Throwable");
        }
        throw v;
    };
    R.catchMatch = function (err, typeNames, callerScopeCls) {
        if (!err || err.__php !== 2) return false;
        for (let i = 0; i < typeNames.length; i++) {
            const target = resolveClass(typeNames[i], callerScopeCls, callerScopeCls, false);
            if (target && isSubtype(err.cls, target)) return true;
        }
        return false;
    };
    R.matchErr = function (val) {
        rt.throwError("UnhandledMatchError", "Unhandled match case " + rt.toStr(val));
    };

    // ---- Basic Reflection (ReflectionClass, ReflectionMethod, ReflectionAttribute) ----
    const RefAttrClass = makeClassMeta({
        name: "ReflectionAttribute", kind: "class",
        methods: [
            { name: "getName", vis: 0, builtin: function () { return this.__meta.name; } },
            { name: "getArguments", vis: 0, builtin: function () { return rt.cloneArr(this.__meta.args); } },
            { name: "newInstance", vis: 0, builtin: function () {
                const args = [], named = {};
                let hasNamed = false;
                for (const [k, v] of this.__meta.args.m) {
                    if (typeof k === "string") { named[k] = v; hasNamed = true; } else args.push(v);
                }
                return R.newObj(this.__meta.name, args, hasNamed ? named : null, null, null, false);
            }},
        ]
    });
    function wrapAttrs(list, filterName) {
        const out = rt.makeArr();
        for (const a of (list || [])) {
            if (filterName && canon(a.name) !== canon(rt.toStr(filterName))) continue;
            const o = allocInstance(RefAttrClass);
            o.__meta = a;
            rt.arrPushInPlace(out, o);
        }
        return out;
    }
    const RefMethodClass = makeClassMeta({
        name: "ReflectionMethod", kind: "class",
        methods: [
            { name: "getName", vis: 0, builtin: function () { return this.__meta.name; } },
            { name: "isPublic", vis: 0, builtin: function () { return this.__meta.vis === 0; } },
            { name: "isProtected", vis: 0, builtin: function () { return this.__meta.vis === 1; } },
            { name: "isPrivate", vis: 0, builtin: function () { return this.__meta.vis === 2; } },
            { name: "isStatic", vis: 0, builtin: function () { return !!this.__meta.isStatic; } },
            { name: "getAttributes", vis: 0, builtin: function (n) { return wrapAttrs(this.__meta.attrs, n); } },
            { name: "invoke", vis: 0, builtin: function (obj) {
                const rest = Array.from(arguments).slice(1);
                return invokeMethod(rt.deref(obj), this.__meta, rest, null, this.__meta.owner, this.__meta.owner, false);
            }},
        ]
    });
    makeClassMeta({
        name: "ReflectionClass", kind: "class",
        methods: [
            { name: "__construct", vis: 0, builtin: function (target) { this.__meta = requireClass(target, null, null); } },
            { name: "getName", vis: 0, builtin: function () { return this.__meta.name; } },
            { name: "getShortName", vis: 0, builtin: function () { const p = this.__meta.name.split("\\"); return p[p.length - 1]; } },
            { name: "getNamespaceName", vis: 0, builtin: function () { const i = this.__meta.name.lastIndexOf("\\"); return i >= 0 ? this.__meta.name.slice(0, i) : ""; } },
            { name: "hasMethod", vis: 0, builtin: function (n) { return this.__meta.methods.has(rt.toStr(n).toLowerCase()); } },
            { name: "getMethod", vis: 0, builtin: function (n) {
                const m = this.__meta.methods.get(rt.toStr(n).toLowerCase());
                if (!m) rt.throwError("ReflectionException", "Method does not exist");
                const o = allocInstance(RefMethodClass); o.__meta = m; return o;
            }},
            { name: "getMethods", vis: 0, builtin: function () {
                const out = rt.makeArr();
                for (const m of this.__meta.methods.values()) { const o = allocInstance(RefMethodClass); o.__meta = m; rt.arrPushInPlace(out, o); }
                return out;
            }},
            { name: "getAttributes", vis: 0, builtin: function (n) { return wrapAttrs(this.__meta.attrs, n); } },
            { name: "newInstanceArgs", vis: 0, builtin: function (arr) {
                const list = arr ? Array.from(rt.toArray(arr).m.values()).map(rt.deref) : [];
                return R.newObj(this.__meta, list, null, null, null, false);
            }},
            { name: "implementsInterface", vis: 0, builtin: function (n) {
                const t = resolveClass(n, null, null, true);
                return !!(t && isSubtype(this.__meta, t));
            }},
        ]
    });

    // ---- Reflection / SPL / OOP built-in functions ----
    const reg = rt.regFn;
    reg("spl_autoload_register", function (cb) { autoloaders.push(cb); return true; });
    reg("spl_autoload_unregister", function (cb) {
        const i = autoloaders.indexOf(cb);
        if (i >= 0) { autoloaders.splice(i, 1); return true; }
        return false;
    });
    reg("spl_object_id", function (o) {
        o = rt.deref(o);
        if (!o || (o.__php !== 2 && o.__php !== 3)) rt.throwError("TypeError", "spl_object_id(): Argument #1 ($object) must be of type object");
        return BigInt(o.id || 0);
    });
    reg("class_exists", function (n, al) {
        const c = resolveClass(n, null, null, al === undefined ? true : rt.truthy(al));
        return !!(c && (c.kind === "class" || c.kind === "enum"));
    });
    reg("interface_exists", function (n, al) {
        const c = resolveClass(n, null, null, al === undefined ? true : rt.truthy(al));
        return !!(c && c.kind === "interface");
    });
    reg("trait_exists", function (n, al) {
        const c = resolveClass(n, null, null, al === undefined ? true : rt.truthy(al));
        return !!(c && c.kind === "trait");
    });
    reg("enum_exists", function (n, al) {
        const c = resolveClass(n, null, null, al === undefined ? true : rt.truthy(al));
        return !!(c && c.kind === "enum");
    });
    reg("function_exists", function (n) { return rt.funcs.has(canon(rt.toStr(n))); });
    reg("method_exists", function (objOrCls, m) {
        const c = resolveClass(objOrCls, null, null, true);
        return !!(c && c.methods.has(rt.toStr(m).toLowerCase()));
    });
    reg("property_exists", function (objOrCls, p) {
        const v = rt.deref(objOrCls);
        const pn = rt.toStr(p);
        if (v && v.__php === 2) {
            if (v.props[pn] !== undefined || v.cls.props.has(pn) || v.cls.props.has("\0" + v.cls.name + "\0" + pn)) return true;
            return v.cls.staticProps.has(pn);
        }
        const c = resolveClass(v, null, null, true);
        return !!(c && (c.props.has(pn) || c.props.has("\0" + c.name + "\0" + pn) || c.staticProps.has(pn)));
    });
    reg("get_class", function (o) {
        o = rt.deref(o);
        if (o && o.__php === 2) return o.cls.name;
        if (o && o.__php === 3) return "Closure";
        rt.throwError("TypeError", "get_class(): Argument #1 ($object) must be of type object");
    });
    reg("get_parent_class", function (o) {
        const c = resolveClass(o, null, null, true);
        return (c && c.parent) ? c.parent.name : false;
    });
    reg("is_a", function (objOrStr, clsName, allowStr) {
        const v = rt.deref(objOrStr);
        let c = null;
        if (v && v.__php === 2) c = v.cls;
        else if (v && v.__php === 3) c = ClosureClass;
        else if (allowStr && rt.truthy(allowStr) && typeof v === "string") c = resolveClass(v, null, null, true);
        const target = resolveClass(clsName, null, null, false);
        return !!(c && target && isSubtype(c, target));
    });
    reg("is_subclass_of", function (objOrStr, clsName, allowStr) {
        const v = rt.deref(objOrStr);
        let c = null;
        if (v && v.__php === 2) c = v.cls;
        else if (typeof v === "string" && (allowStr === undefined || rt.truthy(allowStr))) c = resolveClass(v, null, null, true);
        const target = resolveClass(clsName, null, null, false);
        return !!(c && target && c !== target && isSubtype(c, target));
    });
    reg("get_object_vars", function (o) {
        o = rt.deref(o);
        if (!o || o.__php !== 2) rt.throwError("TypeError", "get_object_vars(): Argument #1 ($object) must be of type object");
        const out = rt.makeArr();
        for (const k in o.props) if (k.charCodeAt(0) !== 0) rt.arrSetInPlace(out, k, o.props[k]);
        return out;
    });
    reg("call_user_func", function (cb) {
        return callAny(cb, Array.from(arguments).slice(1), null, null, false);
    });
    reg("call_user_func_array", function (cb, argsArr) {
        const a = rt.toArray(argsArr);
        const pos = [], named = {};
        let hasNamed = false;
        for (const [k, v] of a.m) {
            if (typeof k === "string") { named[k] = v; hasNamed = true; }
            else pos.push(v);
        }
        return callAny(cb, pos, hasNamed ? named : null, null, false);
    });
})(__zipp_php);
