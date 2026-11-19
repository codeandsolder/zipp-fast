/* ZIPP PHP runtime — core standard library (strings, math, var, JSON, PCRE, DateTime,
 * crypto, filesystem/streams, output buffering, filter_var). Apache-2.0. */
(function (R) {
    "use strict";
    const rt = R.__rt;
    const reg = rt.regFn;

    // ---- Standard PHP constants ----
    function defC(k, v) { rt.consts.set(k, v); }
    defC("PHP_EOL", "\n");
    defC("PHP_INT_MAX", rt.I64_MAX);
    defC("PHP_INT_MIN", rt.I64_MIN);
    defC("PHP_INT_SIZE", 8n);
    defC("PHP_FLOAT_MAX", Number.MAX_VALUE);
    defC("PHP_FLOAT_MIN", 2.2250738585072014e-308);
    defC("PHP_FLOAT_EPSILON", Number.EPSILON);
    defC("PHP_VERSION", "8.4.15-zipp");
    defC("PHP_MAJOR_VERSION", 8n);
    defC("PHP_MINOR_VERSION", 4n);
    defC("PHP_RELEASE_VERSION", 15n);
    defC("PHP_VERSION_ID", 80415n);
    defC("PHP_SAPI", "cli");
    defC("DIRECTORY_SEPARATOR", "/");
    defC("PATH_SEPARATOR", ":");
    defC("INF", Infinity);
    defC("NAN", NaN);
    defC("M_PI", Math.PI);
    defC("M_E", Math.E);
    defC("M_LN2", Math.LN2);
    defC("M_LN10", Math.LN10);
    defC("M_SQRT2", Math.SQRT2);
    defC("STR_PAD_RIGHT", 1n);
    defC("STR_PAD_LEFT", 0n);
    defC("STR_PAD_BOTH", 2n);
    defC("ENT_COMPAT", 2n);
    defC("ENT_QUOTES", 3n);
    defC("ENT_NOQUOTES", 0n);
    defC("ENT_SUBSTITUTE", 8n);
    defC("ENT_HTML401", 0n);
    defC("ENT_HTML5", 48n);
    defC("JSON_HEX_TAG", 1n);
    defC("JSON_HEX_AMP", 2n);
    defC("JSON_HEX_APOS", 4n);
    defC("JSON_HEX_QUOT", 8n);
    defC("JSON_FORCE_OBJECT", 16n);
    defC("JSON_NUMERIC_CHECK", 32n);
    defC("JSON_UNESCAPED_SLASHES", 64n);
    defC("JSON_PRETTY_PRINT", 128n);
    defC("JSON_UNESCAPED_UNICODE", 256n);
    defC("JSON_THROW_ON_ERROR", 4194304n);
    defC("JSON_ERROR_NONE", 0n);
    defC("JSON_ERROR_SYNTAX", 4n);
    defC("PREG_PATTERN_ORDER", 1n);
    defC("PREG_SET_ORDER", 2n);
    defC("PREG_OFFSET_CAPTURE", 256n);
    defC("PREG_UNMATCHED_AS_NULL", 512n);
    defC("PREG_SPLIT_NO_EMPTY", 1n);
    defC("PREG_SPLIT_DELIM_CAPTURE", 2n);
    defC("PREG_SPLIT_OFFSET_CAPTURE", 4n);
    defC("ARRAY_FILTER_USE_BOTH", 1n);
    defC("ARRAY_FILTER_USE_KEY", 2n);
    defC("CASE_LOWER", 0n);
    defC("CASE_UPPER", 1n);
    defC("SORT_REGULAR", 0n);
    defC("SORT_NUMERIC", 1n);
    defC("SORT_STRING", 2n);
    defC("SORT_ASC", 4n);
    defC("SORT_DESC", 3n);
    defC("PHP_ROUND_HALF_UP", 1n);
    defC("PHP_ROUND_HALF_DOWN", 2n);
    defC("PHP_ROUND_HALF_EVEN", 3n);
    defC("PHP_ROUND_HALF_ODD", 4n);
    defC("E_ERROR", 1n);
    defC("E_WARNING", 2n);
    defC("E_PARSE", 4n);
    defC("E_NOTICE", 8n);
    defC("E_USER_ERROR", 256n);
    defC("E_USER_WARNING", 512n);
    defC("E_USER_NOTICE", 1024n);
    defC("E_DEPRECATED", 8192n);
    defC("E_ALL", 32767n);
    defC("FILE_USE_INCLUDE_PATH", 1n);
    defC("FILE_IGNORE_NEW_LINES", 2n);
    defC("FILE_SKIP_EMPTY_LINES", 4n);
    defC("FILE_APPEND", 8n);
    defC("LOCK_EX", 2n);
    defC("SEEK_SET", 0n);
    defC("SEEK_CUR", 1n);
    defC("SEEK_END", 2n);
    defC("PATHINFO_DIRNAME", 1n);
    defC("PATHINFO_BASENAME", 2n);
    defC("PATHINFO_EXTENSION", 4n);
    defC("PATHINFO_FILENAME", 8n);
    defC("PHP_URL_SCHEME", 0n);
    defC("PHP_URL_HOST", 1n);
    defC("PHP_URL_PORT", 2n);
    defC("PHP_URL_USER", 3n);
    defC("PHP_URL_PASS", 4n);
    defC("PHP_URL_PATH", 5n);
    defC("PHP_URL_QUERY", 6n);
    defC("PHP_URL_FRAGMENT", 7n);
    defC("PASSWORD_DEFAULT", "2y");
    defC("PASSWORD_BCRYPT", "2y");
    defC("FILTER_VALIDATE_INT", 257n);
    defC("FILTER_VALIDATE_BOOLEAN", 258n);
    defC("FILTER_VALIDATE_BOOL", 258n);
    defC("FILTER_VALIDATE_FLOAT", 259n);
    defC("FILTER_VALIDATE_URL", 273n);
    defC("FILTER_VALIDATE_EMAIL", 274n);
    defC("FILTER_VALIDATE_IP", 275n);
    defC("PHP_SESSION_DISABLED", 0n);
    defC("PHP_SESSION_NONE", 1n);
    defC("PHP_SESSION_ACTIVE", 2n);

    // ---- Types, constants, inspection ----
    function getTypeStr(v) {
        v = rt.deref(v);
        if (v === null || v === undefined) return "NULL";
        if (typeof v === "boolean") return "boolean";
        if (typeof v === "bigint") return "integer";
        if (typeof v === "number") return "double";
        if (typeof v === "string") return "string";
        if (v.__php === 1) return "array";
        if (v.__php === 2 || v.__php === 3) return "object";
        if (v.__php === 4) return v.closed ? "resource (closed)" : "resource";
        return "unknown type";
    }
    function getDebugType(v) {
        v = rt.deref(v);
        if (v === null || v === undefined) return "null";
        if (typeof v === "boolean") return "bool";
        if (typeof v === "bigint") return "int";
        if (typeof v === "number") return "float";
        if (typeof v === "string") return "string";
        if (v.__php === 1) return "array";
        if (v.__php === 2) return v.cls.name;
        if (v.__php === 3) return "Closure";
        if (v.__php === 4) return v.closed ? "resource (closed)" : "resource (" + v.kind + ")";
        return "unknown";
    }
    rt.getDebugType = getDebugType;

    reg("gettype", getTypeStr);
    reg("get_debug_type", getDebugType);
    reg("is_int", function (v) { return typeof rt.deref(v) === "bigint"; });
    reg("is_integer", function (v) { return typeof rt.deref(v) === "bigint"; });
    reg("is_long", function (v) { return typeof rt.deref(v) === "bigint"; });
    reg("is_float", function (v) { return typeof rt.deref(v) === "number"; });
    reg("is_double", function (v) { return typeof rt.deref(v) === "number"; });
    reg("is_real", function (v) { return typeof rt.deref(v) === "number"; });
    reg("is_string", function (v) { return typeof rt.deref(v) === "string"; });
    reg("is_bool", function (v) { return typeof rt.deref(v) === "boolean"; });
    reg("is_null", function (v) { v = rt.deref(v); return v === null || v === undefined; });
    R.isArr = function (v) {
        if (v !== null && typeof v === "object") {
            if (v.__php === 5) v = v.v;
            return v !== null && typeof v === "object" && v.__php === 1;
        }
        return false;
    };
    R.strlen = function (s) {
        if (typeof s === "string") return BigInt(s.length);
        return BigInt(rt.toStr(s).length);
    };
    R.count = function (v) {
        if (v !== null && typeof v === "object") {
            if (v.__php === 1) return BigInt(v.m.size);
            if (v.__php === 5 && v.v && v.v.__php === 1) return BigInt(v.v.m.size);
        }
        return rt.funcs.get("count").builtin(v);
    };
    reg("is_object", function (v) { v = rt.deref(v); return !!(v && (v.__php === 2 || v.__php === 3)); });
    reg("is_resource", function (v) { v = rt.deref(v); return !!(v && v.__php === 4 && !v.closed); });
    reg("is_callable", function (v) { return rt.isCallable(v); });
    reg("is_scalar", function (v) {
        v = rt.deref(v);
        return typeof v === "bigint" || typeof v === "number" || typeof v === "string" || typeof v === "boolean";
    });
    reg("is_numeric", function (v) {
        v = rt.deref(v);
        if (typeof v === "bigint" || typeof v === "number") return true;
        if (typeof v === "string") return rt.parseNumericString(v) !== null;
        return false;
    });
    reg("is_iterable", function (v) {
        v = rt.deref(v);
        return !!(v && (v.__php === 1 || (v.__php === 2 && rt.isSubtype(v.cls, rt.classes.get("traversable")))));
    });
    reg("intval", function (v, base) {
        v = rt.deref(v);
        if (base !== undefined && typeof v === "string") {
            const b = Number(rt.toInt(base));
            const n = parseInt(v.trim(), b);
            return Number.isNaN(n) ? 0n : BigInt(n);
        }
        return rt.toInt(v);
    });
    reg("floatval", function (v) { return rt.toFloat(v); });
    reg("doubleval", function (v) { return rt.toFloat(v); });
    reg("strval", function (v) { return rt.toStr(v); });
    reg("boolval", function (v) { return rt.truthy(v); });
    reg("define", function (name, val) {
        const n = rt.toStr(name).replace(/^\\/, "");
        if (rt.consts.has(n)) return false;
        rt.consts.set(n, rt.share(rt.deref(val)));
        return true;
    });
    reg("defined", function (name) {
        const n = rt.toStr(name).replace(/^\\/, "");
        if (n.includes("::")) {
            const [c, k] = n.split("::");
            const cls = rt.resolveClass(c, null, null, true);
            return !!(cls && cls.consts.has(k));
        }
        return rt.consts.has(n);
    });
    reg("assert", function (assertion, description) {
        if (rt.truthy(assertion)) return true;
        if (description && description.__php === 2) throw description;
        rt.throwError("AssertionError", description !== undefined && description !== null ? rt.toStr(description) : "assert() failed");
    });
    reg("constant", function (name) {
        const n = rt.toStr(name).replace(/^\\/, "");
        if (n.includes("::")) {
            const [c, k] = n.split("::");
            return R.staticConst(c, k, null, null);
        }
        return R.getConst(n, null);
    });

    // ---- var_dump / print_r / var_export / serialize / unserialize ----
    function formatVarDump(v, indent) {
        v = rt.deref(v);
        const pad = " ".repeat(indent);
        if (v === null || v === undefined) return pad + "NULL\n";
        if (typeof v === "boolean") return pad + "bool(" + (v ? "true" : "false") + ")\n";
        if (typeof v === "bigint") return pad + "int(" + v.toString() + ")\n";
        if (typeof v === "number") return pad + "float(" + rt.formatFloat(v) + ")\n";
        if (typeof v === "string") return pad + "string(" + v.length + ') "' + v + '"\n';
        if (v.__php === 1) {
            let out = pad + "array(" + v.m.size + ") {\n";
            for (const [k, val] of v.m) {
                const ks = typeof k === "bigint" ? k.toString() : ('"' + k + '"');
                out += pad + "  [" + ks + "]=>\n" + formatVarDump(val, indent + 2);
            }
            return out + pad + "}\n";
        }
        if (v.__php === 2) {
            if (v.enumCase !== undefined) {
                return pad + "enum(" + v.cls.name + "::" + v.enumCase + ")\n";
            }
            const pkeys = Object.keys(v.props);
            let out = pad + "object(" + v.cls.name + ")#" + v.id + " (" + pkeys.length + ") {\n";
            for (let i = 0; i < pkeys.length; i++) {
                const k = pkeys[i], val = v.props[k];
                let label;
                if (k.charCodeAt(0) === 0) {
                    const parts = k.slice(1).split("\0");
                    label = '"' + parts[1] + '":"' + parts[0] + '":private';
                } else {
                    label = '"' + k + '"';
                }
                out += pad + "  [" + label + "]=>\n" + formatVarDump(val, indent + 2);
            }
            return out + pad + "}\n";
        }
        return pad + "object(Closure)#1 (0) {\n" + pad + "}\n";
    }
    reg("var_dump", function () {
        for (let i = 0; i < arguments.length; i++) rt.echoStr(formatVarDump(arguments[i], 0));
        return null;
    });

    function formatPrintR(v, indent) {
        v = rt.deref(v);
        if (v === null || v === undefined || v === false) return "";
        if (v === true) return "1";
        if (typeof v === "bigint") return v.toString();
        if (typeof v === "number") return rt.formatFloat(v);
        if (typeof v === "string") return v;
        const pad = " ".repeat(indent);
        if (v.__php === 1) {
            let out = "Array\n" + pad + "(\n";
            for (const [k, val] of v.m) {
                out += pad + "    [" + String(k) + "] => " + formatPrintR(val, indent + 8) + "\n";
            }
            return out + pad + ")\n";
        }
        if (v.__php === 2) {
            let out = v.cls.name + " Object\n" + pad + "(\n";
            for (const k in v.props) {
                const label = k.charCodeAt(0) === 0 ? k.slice(1).split("\0")[1] + ":" + k.slice(1).split("\0")[0] + ":private" : k;
                out += pad + "    [" + label + "] => " + formatPrintR(v.props[k], indent + 8) + "\n";
            }
            return out + pad + ")\n";
        }
        return "Closure Object\n(\n)\n";
    }
    reg("print_r", function (v, ret) {
        const s = formatPrintR(v, 0);
        if (ret !== undefined && rt.truthy(ret)) return s;
        rt.echoStr(s);
        return true;
    });

    function formatVarExport(v, indent) {
        v = rt.deref(v);
        if (v === null || v === undefined) return "NULL";
        if (typeof v === "boolean") return v ? "true" : "false";
        if (typeof v === "bigint") return v.toString();
        if (typeof v === "number") {
            const s = rt.formatFloat(v);
            return s.includes(".") || s.includes("E") ? s : s + ".0";
        }
        if (typeof v === "string") return "'" + v.replace(/\\/g, "\\\\").replace(/'/g, "\\'") + "'";
        if (v.__php === 1) {
            const pad = " ".repeat(indent);
            let out = "array (\n";
            for (const [k, val] of v.m) {
                const ks = typeof k === "bigint" ? k.toString() : formatVarExport(k, 0);
                out += pad + "  " + ks + " => " + formatVarExport(val, indent + 2) + ",\n";
            }
            return out + pad + ")";
        }
        return "NULL";
    }
    reg("var_export", function (v, ret) {
        const s = formatVarExport(v, 0);
        if (ret !== undefined && rt.truthy(ret)) return s;
        rt.echoStr(s);
        return null;
    });

    function phpSerialize(v) {
        v = rt.deref(v);
        if (v === null || v === undefined) return "N;";
        if (typeof v === "boolean") return "b:" + (v ? "1" : "0") + ";";
        if (typeof v === "bigint") return "i:" + v.toString() + ";";
        if (typeof v === "number") return "d:" + rt.formatFloat(v) + ";";
        if (typeof v === "string") return "s:" + v.length + ':"' + v + '";';
        if (v.__php === 1) {
            let out = "a:" + v.m.size + ":{";
            for (const [k, val] of v.m) out += phpSerialize(k) + phpSerialize(val);
            return out + "}";
        }
        if (v.__php === 2) {
            const pkeys = Object.keys(v.props);
            let out = "O:" + v.cls.name.length + ':"' + v.cls.name + '":' + pkeys.length + ":{";
            for (let i = 0; i < pkeys.length; i++) out += phpSerialize(pkeys[i]) + phpSerialize(v.props[pkeys[i]]);
            return out + "}";
        }
        return "N;";
    }
    function phpUnserialize(s) {
        let p = 0;
        function readUntil(ch) {
            const i = s.indexOf(ch, p);
            if (i < 0) throw new Error("bad serialize");
            const r = s.slice(p, i);
            p = i + 1;
            return r;
        }
        function parse() {
            const t = s[p++];
            if (t === "N") { p++; return null; }
            p++; // ':'
            if (t === "b") { const v = readUntil(";"); return v === "1"; }
            if (t === "i") { return BigInt(readUntil(";")); }
            if (t === "d") { return Number(readUntil(";")); }
            if (t === "s") {
                const len = Number(readUntil(":"));
                p++; // '"'
                const str = s.slice(p, p + len);
                p += len + 2; // '";'
                return str;
            }
            if (t === "a") {
                const n = Number(readUntil(":"));
                p++; // '{'
                const arr = rt.makeArr();
                for (let i = 0; i < n; i++) {
                    const k = parse(), v = parse();
                    rt.arrSetInPlace(arr, k, v);
                }
                p++; // '}'
                return arr;
            }
            if (t === "O") {
                const clen = Number(readUntil(":"));
                p++; // '"'
                const cname = s.slice(p, p + clen);
                p += clen + 2; // '":'
                const n = Number(readUntil(":"));
                p++; // '{'
                const cls = rt.resolveClass(cname, null, null, true) || rt.classes.get("stdclass");
                const obj = rt.allocInstance(cls);
                for (let i = 0; i < n; i++) {
                    const k = rt.toStr(parse()), v = parse();
                    obj.props[k] = v;
                }
                p++; // '}'
                return obj;
            }
            throw new Error("bad serialize tag");
        }
        try { return parse(); } catch (_) { return false; }
    }
    reg("serialize", phpSerialize);
    reg("unserialize", function (s) { return phpUnserialize(rt.toStr(s)); });

    // ---- Strings (60+ core PHP functions) ----
    reg("strlen", function (s) { return BigInt(rt.toStr(s).length); });
    reg("substr", function (s, offset, length) {
        s = rt.toStr(s);
        const n = s.length;
        let start = Number(rt.toInt(offset));
        if (start < 0) start = Math.max(0, n + start);
        if (start >= n) return "";
        if (length === undefined || rt.deref(length) === null) return s.slice(start);
        const len = Number(rt.toInt(length));
        if (len < 0) {
            const end = n + len;
            return end <= start ? "" : s.slice(start, end);
        }
        return s.slice(start, start + len);
    });
    reg("strpos", function (hay, needle, offset) {
        hay = rt.toStr(hay); needle = rt.toStr(needle);
        let off = offset !== undefined ? Number(rt.toInt(offset)) : 0;
        if (off < 0) off = Math.max(0, hay.length + off);
        const i = hay.indexOf(needle, off);
        return i < 0 ? false : BigInt(i);
    });
    reg("stripos", function (hay, needle, offset) {
        hay = rt.toStr(hay).toLowerCase(); needle = rt.toStr(needle).toLowerCase();
        let off = offset !== undefined ? Number(rt.toInt(offset)) : 0;
        if (off < 0) off = Math.max(0, hay.length + off);
        const i = hay.indexOf(needle, off);
        return i < 0 ? false : BigInt(i);
    });
    reg("strrpos", function (hay, needle, offset) {
        hay = rt.toStr(hay); needle = rt.toStr(needle);
        let off = offset !== undefined ? Number(rt.toInt(offset)) : 0;
        let i;
        if (off < 0) {
            const limit = hay.length + off;
            i = limit < 0 ? -1 : hay.lastIndexOf(needle, limit);
        } else {
            i = hay.lastIndexOf(needle);
            if (i < off) i = -1;
        }
        return i < 0 ? false : BigInt(i);
    });
    reg("str_contains", function (h, n) { return rt.toStr(h).includes(rt.toStr(n)); });
    reg("str_starts_with", function (h, n) { return rt.toStr(h).startsWith(rt.toStr(n)); });
    reg("str_ends_with", function (h, n) { return rt.toStr(h).endsWith(rt.toStr(n)); });

    function strReplaceImpl(search, replace, subject, ci, countRef) {
        let total = 0;
        function repOne(subj, sList, rList) {
            let out = rt.toStr(subj);
            for (let i = 0; i < sList.length; i++) {
                const needle = rt.toStr(sList[i]);
                if (needle.length === 0) continue;
                const rep = i < rList.length ? rt.toStr(rList[i]) : (rList.length === 1 && !Array.isArray(replace) ? rt.toStr(rList[0]) : "");
                if (!ci) {
                    const parts = out.split(needle);
                    total += parts.length - 1;
                    out = parts.join(rep);
                } else {
                    const low = out.toLowerCase(), nlow = needle.toLowerCase();
                    let res = "", pos = 0;
                    for (;;) {
                        const idx = low.indexOf(nlow, pos);
                        if (idx < 0) { res += out.slice(pos); break; }
                        res += out.slice(pos, idx) + rep;
                        pos = idx + needle.length;
                        total++;
                    }
                    out = res;
                }
            }
            return out;
        }
        search = rt.deref(search); replace = rt.deref(replace); subject = rt.deref(subject);
        const sArr = search && search.__php === 1 ? Array.from(search.m.values()) : [search];
        const rIsArr = replace && replace.__php === 1;
        const rArr = rIsArr ? Array.from(replace.m.values()) : sArr.map(() => replace);
        let result;
        if (subject && subject.__php === 1) {
            result = rt.makeArr();
            for (const [k, v] of subject.m) rt.arrSetInPlace(result, k, repOne(v, sArr, rArr));
        } else {
            result = repOne(subject, sArr, rArr);
        }
        if (countRef && rt.isRef(countRef)) countRef.v = BigInt(total);
        return result;
    }
    reg("str_replace", function (s, r, subj, cnt) { return strReplaceImpl(s, r, subj, false, cnt); }, 8);
    reg("str_ireplace", function (s, r, subj, cnt) { return strReplaceImpl(s, r, subj, true, cnt); }, 8);

    reg("explode", function (sep, str, limit) {
        sep = rt.toStr(sep); str = rt.toStr(str);
        if (sep.length === 0) rt.throwError("ValueError", "explode(): Argument #1 ($separator) cannot be empty");
        const out = rt.makeArr();
        const parts = str.split(sep);
        if (limit === undefined) {
            for (const p of parts) rt.arrPushInPlace(out, p);
            return out;
        }
        const lim = Number(rt.toInt(limit));
        if (lim > 0) {
            if (parts.length <= lim) { for (const p of parts) rt.arrPushInPlace(out, p); }
            else {
                for (let i = 0; i < lim - 1; i++) rt.arrPushInPlace(out, parts[i]);
                rt.arrPushInPlace(out, parts.slice(lim - 1).join(sep));
            }
        } else if (lim < 0) {
            const take = Math.max(0, parts.length + lim);
            for (let i = 0; i < take; i++) rt.arrPushInPlace(out, parts[i]);
        } else {
            rt.arrPushInPlace(out, str);
        }
        return out;
    });
    function implodeFn(a, b) {
        let sep = "", arr;
        if (arguments.length === 1) { arr = rt.toArray(a); }
        else { sep = rt.toStr(a); arr = rt.toArray(b); }
        const parts = [];
        for (const v of arr.m.values()) parts.push(rt.toStr(v));
        return parts.join(sep);
    }
    reg("implode", implodeFn);
    reg("join", implodeFn);

    function expandTrimChars(chars) {
        const set = new Set();
        for (let i = 0; i < chars.length; i++) {
            if (i + 3 < chars.length && chars[i + 1] === "." && chars[i + 2] === ".") {
                const a = chars.charCodeAt(i), b = chars.charCodeAt(i + 3);
                for (let c = a; c <= b; c++) set.add(c);
                i += 3;
            } else {
                set.add(chars.charCodeAt(i));
            }
        }
        return set;
    }
    const DEFAULT_TRIM = expandTrimChars(" \t\n\r\0\x0B");
    function doTrim(s, chars, left, right) {
        s = rt.toStr(s);
        const set = chars === undefined ? DEFAULT_TRIM : expandTrimChars(rt.toStr(chars));
        let i = 0, j = s.length;
        if (left) while (i < j && set.has(s.charCodeAt(i))) i++;
        if (right) while (j > i && set.has(s.charCodeAt(j - 1))) j--;
        return s.slice(i, j);
    }
    reg("trim", function (s, c) { return doTrim(s, c, true, true); });
    reg("ltrim", function (s, c) { return doTrim(s, c, true, false); });
    reg("rtrim", function (s, c) { return doTrim(s, c, false, true); });
    reg("chop", function (s, c) { return doTrim(s, c, false, true); });

    reg("strtolower", function (s) {
        s = rt.toStr(s);
        let out = "";
        for (let i = 0; i < s.length; i++) {
            const c = s.charCodeAt(i);
            out += (c >= 65 && c <= 90) ? String.fromCharCode(c + 32) : s[i];
        }
        return out;
    });
    reg("strtoupper", function (s) {
        s = rt.toStr(s);
        let out = "";
        for (let i = 0; i < s.length; i++) {
            const c = s.charCodeAt(i);
            out += (c >= 97 && c <= 122) ? String.fromCharCode(c - 32) : s[i];
        }
        return out;
    });
    reg("ucfirst", function (s) {
        s = rt.toStr(s);
        if (!s.length) return "";
        const c = s.charCodeAt(0);
        return (c >= 97 && c <= 122 ? String.fromCharCode(c - 32) : s[0]) + s.slice(1);
    });
    reg("lcfirst", function (s) {
        s = rt.toStr(s);
        if (!s.length) return "";
        const c = s.charCodeAt(0);
        return (c >= 65 && c <= 90 ? String.fromCharCode(c + 32) : s[0]) + s.slice(1);
    });
    reg("ucwords", function (s, seps) {
        s = rt.toStr(s);
        const delims = new Set((seps === undefined ? " \t\r\n\f\v" : rt.toStr(seps)).split(""));
        let out = "", cap = true;
        for (let i = 0; i < s.length; i++) {
            const ch = s[i];
            if (delims.has(ch)) { out += ch; cap = true; }
            else if (cap) {
                const c = ch.charCodeAt(0);
                out += (c >= 97 && c <= 122) ? String.fromCharCode(c - 32) : ch;
                cap = false;
            } else out += ch;
        }
        return out;
    });
    reg("str_repeat", function (s, n) {
        const t = Number(rt.toInt(n));
        if (t < 0) rt.throwError("ValueError", "str_repeat(): Argument #2 ($times) must be greater than or equal to 0");
        return rt.toStr(s).repeat(t);
    });
    reg("str_pad", function (s, len, pad, type) {
        s = rt.toStr(s);
        const target = Number(rt.toInt(len));
        if (target <= s.length) return s;
        const p = pad === undefined ? " " : rt.toStr(pad);
        if (p.length === 0) rt.throwError("ValueError", "str_pad(): Argument #3 ($pad_string) must be a non-empty string");
        const mode = type === undefined ? 1 : Number(rt.toInt(type));
        const need = target - s.length;
        const fill = function (n) { return p.repeat(Math.ceil(n / p.length)).slice(0, n); };
        if (mode === 0) return fill(need) + s;
        if (mode === 2) { const l = Math.floor(need / 2); return fill(l) + s + fill(need - l); }
        return s + fill(need);
    });
    reg("str_split", function (s, len) {
        s = rt.toStr(s);
        const n = len === undefined ? 1 : Number(rt.toInt(len));
        if (n < 1) rt.throwError("ValueError", "str_split(): Argument #2 ($length) must be greater than 0");
        const out = rt.makeArr();
        if (s.length === 0) { rt.arrPushInPlace(out, ""); return out; }
        for (let i = 0; i < s.length; i += n) rt.arrPushInPlace(out, s.slice(i, i + n));
        return out;
    });
    reg("strrev", function (s) { return rt.toStr(s).split("").reverse().join(""); });
    reg("substr_count", function (h, n, off, len) {
        h = rt.toStr(h); n = rt.toStr(n);
        if (n.length === 0) rt.throwError("ValueError", "substr_count(): Argument #2 ($needle) cannot be empty");
        const sub = off !== undefined ? rt.funcs.get("substr").builtin(h, off, len) : h;
        let c = 0n, pos = 0;
        for (;;) { const i = sub.indexOf(n, pos); if (i < 0) break; c++; pos = i + n.length; }
        return c;
    });
    reg("strcmp", function (a, b) { a = rt.toStr(a); b = rt.toStr(b); return a < b ? -1n : a > b ? 1n : 0n; });
    reg("strcasecmp", function (a, b) { a = rt.toStr(a).toLowerCase(); b = rt.toStr(b).toLowerCase(); return a < b ? -1n : a > b ? 1n : 0n; });
    reg("strncmp", function (a, b, n) { const k = Number(rt.toInt(n)); a = rt.toStr(a).slice(0, k); b = rt.toStr(b).slice(0, k); return a < b ? -1n : a > b ? 1n : 0n; });
    reg("strstr", function (h, n, before) {
        h = rt.toStr(h); n = rt.toStr(n);
        const i = h.indexOf(n);
        if (i < 0) return false;
        return (before && rt.truthy(before)) ? h.slice(0, i) : h.slice(i);
    });
    reg("strrchr", function (h, n) {
        h = rt.toStr(h); n = rt.toStr(n);
        if (!n.length) return false;
        const i = h.lastIndexOf(n[0]);
        return i < 0 ? false : h.slice(i);
    });
    reg("ord", function (s) { s = rt.toStr(s); return s.length ? BigInt(s.charCodeAt(0) & 0xFF) : 0n; });
    reg("chr", function (c) { return String.fromCharCode(Number(rt.toInt(c) & 0xFFn)); });
    reg("bin2hex", function (s) {
        s = rt.toStr(s);
        let out = "";
        for (let i = 0; i < s.length; i++) out += (s.charCodeAt(i) & 0xFF).toString(16).padStart(2, "0");
        return out;
    });
    reg("hex2bin", function (s) {
        s = rt.toStr(s);
        if (s.length % 2 !== 0) return false;
        let out = "";
        for (let i = 0; i < s.length; i += 2) {
            const v = parseInt(s.slice(i, i + 2), 16);
            if (Number.isNaN(v)) return false;
            out += String.fromCharCode(v);
        }
        return out;
    });

    // Base64 (binary-safe over byte-strings)
    const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    reg("base64_encode", function (s) {
        s = rt.toStr(s);
        let out = "";
        for (let i = 0; i < s.length; i += 3) {
            const a = s.charCodeAt(i) & 0xFF;
            const b = i + 1 < s.length ? s.charCodeAt(i + 1) & 0xFF : 0;
            const c = i + 2 < s.length ? s.charCodeAt(i + 2) & 0xFF : 0;
            out += B64[a >> 2] + B64[((a & 3) << 4) | (b >> 4)]
                + (i + 1 < s.length ? B64[((b & 15) << 2) | (c >> 6)] : "=")
                + (i + 2 < s.length ? B64[c & 63] : "=");
        }
        return out;
    });
    reg("base64_decode", function (s, strict) {
        s = rt.toStr(s).replace(/[\r\n\t ]+/g, "");
        const st = strict !== undefined && rt.truthy(strict);
        let out = "", buf = 0, bits = 0;
        for (let i = 0; i < s.length; i++) {
            const ch = s[i];
            if (ch === "=") break;
            const v = B64.indexOf(ch);
            if (v < 0) { if (st) return false; continue; }
            buf = (buf << 6) | v;
            bits += 6;
            if (bits >= 8) {
                bits -= 8;
                out += String.fromCharCode((buf >> bits) & 0xFF);
            }
        }
        return out;
    });

    // HTML / URL encoding & query strings
    reg("htmlspecialchars", function (s, flags) {
        s = rt.toStr(s);
        const f = flags === undefined ? 3 : Number(rt.toInt(flags));
        let out = s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
        if ((f & 2) !== 0) out = out.replace(/"/g, "&quot;");
        if ((f & 1) !== 0) out = out.replace(/'/g, "&#039;");
        return out;
    });
    reg("htmlspecialchars_decode", function (s) {
        return rt.toStr(s).replace(/&quot;/g, '"').replace(/&#0?39;|&apos;/g, "'").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&");
    });
    reg("htmlentities", function (s, flags) { return rt.funcs.get("htmlspecialchars").builtin(s, flags); });
    reg("nl2br", function (s, xhtml) {
        const br = (xhtml === undefined || rt.truthy(xhtml)) ? "<br />" : "<br>";
        return rt.toStr(s).replace(/\r\n|\n\r|\r|\n/g, m => br + m);
    });
    reg("strip_tags", function (s) { return rt.toStr(s).replace(/<[^>]*>/g, ""); });

    function rawUrlEncode(s) {
        s = rt.toStr(s);
        let out = "";
        for (let i = 0; i < s.length; i++) {
            const c = s.charCodeAt(i) & 0xFF;
            if ((c >= 65 && c <= 90) || (c >= 97 && c <= 122) || (c >= 48 && c <= 57) || c === 45 || c === 95 || c === 46 || c === 126) {
                out += String.fromCharCode(c);
            } else {
                out += "%" + c.toString(16).toUpperCase().padStart(2, "0");
            }
        }
        return out;
    }
    function rawUrlDecode(s) {
        return rt.toStr(s).replace(/%([0-9A-Fa-f]{2})/g, (_, hex) => String.fromCharCode(parseInt(hex, 16)));
    }
    reg("rawurlencode", rawUrlEncode);
    reg("rawurldecode", rawUrlDecode);
    reg("urlencode", function (s) { return rawUrlEncode(s).replace(/%20/g, "+"); });
    reg("urldecode", function (s) { return rawUrlDecode(rt.toStr(s).replace(/\+/g, " ")); });
    rt.urlDecode = function (s) { return rawUrlDecode(String(s).replace(/\+/g, " ")); };

    function buildQueryPairs(val, prefix, out) {
        val = rt.deref(val);
        if (val && val.__php === 1) {
            for (const [k, v] of val.m) {
                const key = prefix ? (prefix + "[" + String(k) + "]") : String(k);
                buildQueryPairs(v, key, out);
            }
        } else if (val !== null && val !== undefined) {
            const sv = typeof val === "boolean" ? (val ? "1" : "0") : rt.toStr(val);
            out.push(rawUrlEncode(prefix).replace(/%20/g, "+") + "=" + rawUrlEncode(sv).replace(/%20/g, "+"));
        }
    }
    reg("http_build_query", function (data, numPfx, argSep) {
        const out = [];
        const sep = argSep === undefined ? "&" : rt.toStr(argSep);
        const a = rt.toArray(data);
        for (const [k, v] of a.m) {
            const pfx = (typeof k === "bigint" && numPfx !== undefined) ? (rt.toStr(numPfx) + k.toString()) : String(k);
            buildQueryPairs(v, pfx, out);
        }
        return out.join(sep);
    });

    function parseQueryString(qs) {
        const out = rt.makeArr();
        if (!qs) return out;
        const parts = qs.split("&");
        for (let i = 0; i < parts.length; i++) {
            if (!parts[i]) continue;
            const eq = parts[i].indexOf("=");
            const rawK = eq >= 0 ? parts[i].slice(0, eq) : parts[i];
            const rawV = eq >= 0 ? parts[i].slice(eq + 1) : "";
            const k = rt.urlDecode(rawK);
            const v = rt.urlDecode(rawV);
            const br = k.indexOf("[");
            if (br > 0 && k.endsWith("]")) {
                const base = k.slice(0, br);
                const inner = k.slice(br + 1, -1);
                const keys = [base, inner === "" ? undefined : inner];
                const updated = R.idxSetPath(out, keys, v);
                out.m = updated.m; out.next = updated.next;
            } else {
                rt.arrSetInPlace(out, k, v);
            }
        }
        return out;
    }
    rt.parseQueryString = parseQueryString;
    reg("parse_str", function (qs, resRef) {
        const parsed = parseQueryString(rt.toStr(qs));
        if (resRef && rt.isRef(resRef)) resRef.v = parsed;
        return null;
    }, 2);

    reg("parse_url", function (url, comp) {
        url = rt.toStr(url);
        const res = rt.makeArr();
        const m = /^(?:([a-zA-Z][a-zA-Z0-9+.-]*):)?(?:\/\/(?:([^:@/?#]*)(?::([^@/?#]*))?@)?([^:/?#]*)(?::(\d+))?)?([^?#]*)(?:\?([^#]*))?(?:#(.*))?$/.exec(url);
        if (!m) return false;
        const fields = [
            ["scheme", m[1]], ["host", m[4]], ["port", m[5] !== undefined ? BigInt(m[5]) : undefined],
            ["user", m[2]], ["pass", m[3]], ["path", m[6] || undefined],
            ["query", m[7]], ["fragment", m[8]]
        ];
        if (comp !== undefined && comp !== null) {
            const idx = Number(rt.toInt(comp));
            if (idx >= 0 && idx < fields.length) return fields[idx][1] === undefined ? null : fields[idx][1];
            return null;
        }
        for (const [k, v] of fields) if (v !== undefined) rt.arrSetInPlace(res, k, v);
        return res;
    });

    // sprintf / printf / vsprintf / number_format
    function doVsprintf(fmt, args) {
        fmt = rt.toStr(fmt);
        let idx = 0;
        return fmt.replace(/%(?:(\d+)\$)?([+\- 0]|'.)?(\d+)?(?:\.(\d+))?([%bcdeEfFgGosuxX])/g, function (_, pos, flags, width, prec, spec) {
            if (spec === "%") return "%";
            const arg = pos !== undefined ? args[Number(pos) - 1] : args[idx++];
            flags = flags || "";
            const left = flags.includes("-");
            const plus = flags.includes("+");
            const padCh = (flags.startsWith("'") && flags.length > 1) ? flags[1] : (flags.includes("0") && !left ? "0" : " ");
            let s;
            if (spec === "s") {
                s = rt.toStr(arg);
                if (prec !== undefined) s = s.slice(0, Number(prec));
            } else if (spec === "d") {
                const n = rt.toInt(arg);
                s = (n >= 0n && plus ? "+" : "") + n.toString();
            } else if (spec === "u") {
                s = BigInt.asUintN(64, rt.toInt(arg)).toString();
            } else if (spec === "x" || spec === "X") {
                s = BigInt.asUintN(64, rt.toInt(arg)).toString(16);
                if (spec === "X") s = s.toUpperCase();
            } else if (spec === "o") {
                s = BigInt.asUintN(64, rt.toInt(arg)).toString(8);
            } else if (spec === "b") {
                s = BigInt.asUintN(64, rt.toInt(arg)).toString(2);
            } else if (spec === "c") {
                s = String.fromCharCode(Number(rt.toInt(arg) & 0xFFn));
            } else if (spec === "f" || spec === "F") {
                const f = rt.toFloat(arg);
                const p = prec !== undefined ? Number(prec) : 6;
                s = (f >= 0 && plus ? "+" : "") + f.toFixed(p);
            } else if (spec === "e" || spec === "E") {
                const f = rt.toFloat(arg);
                const p = prec !== undefined ? Number(prec) : 6;
                s = (f >= 0 && plus ? "+" : "") + f.toExponential(p);
                if (spec === "E") s = s.toUpperCase();
            } else if (spec === "g" || spec === "G") {
                const f = rt.toFloat(arg);
                const p = prec !== undefined ? Math.max(1, Number(prec)) : 6;
                s = Number(f.toPrecision(p)).toString();
                if (spec === "G") s = s.toUpperCase();
            } else {
                s = rt.toStr(arg);
            }
            if (width !== undefined) {
                const w = Number(width);
                if (s.length < w) {
                    const pad = padCh.repeat(w - s.length);
                    if (left) s = s + pad;
                    else if (padCh === "0" && (s.startsWith("-") || s.startsWith("+"))) s = s[0] + pad + s.slice(1);
                    else s = pad + s;
                }
            }
            return s;
        });
    }
    reg("sprintf", function (fmt) { return doVsprintf(fmt, Array.from(arguments).slice(1)); });
    reg("vsprintf", function (fmt, arr) { return doVsprintf(fmt, Array.from(rt.toArray(arr).m.values())); });
    reg("printf", function (fmt) { const s = doVsprintf(fmt, Array.from(arguments).slice(1)); rt.echoStr(s); return BigInt(s.length); });
    reg("vprintf", function (fmt, arr) { const s = doVsprintf(fmt, Array.from(rt.toArray(arr).m.values())); rt.echoStr(s); return BigInt(s.length); });
    reg("number_format", function (num, dec, decSep, thouSep) {
        const f = rt.toFloat(num);
        const d = dec === undefined ? 0 : Math.max(0, Number(rt.toInt(dec)));
        const ds = decSep === undefined ? "." : rt.toStr(decSep);
        const ts = thouSep === undefined ? "," : rt.toStr(thouSep);
        const fixed = f.toFixed(d);
        const parts = fixed.split(".");
        parts[0] = parts[0].replace(/\B(?=(\d{3})+(?!\d))/g, ts);
        return parts.length > 1 ? parts[0] + ds + parts[1] : parts[0];
    });

    // mbstring basics (UTF-8 aware over byte-strings!)
    reg("mb_strlen", function (s) { return BigInt(Array.from(rt.bytesToUtf8(rt.toStr(s))).length); });
    reg("mb_substr", function (s, start, len) {
        const cps = Array.from(rt.bytesToUtf8(rt.toStr(s)));
        let st = Number(rt.toInt(start));
        if (st < 0) st = Math.max(0, cps.length + st);
        if (len === undefined || rt.deref(len) === null) return rt.utf8ToBytes(cps.slice(st).join(""));
        const l = Number(rt.toInt(len));
        const end = l < 0 ? Math.max(st, cps.length + l) : st + l;
        return rt.utf8ToBytes(cps.slice(st, end).join(""));
    });
    reg("mb_strtolower", function (s) { return rt.utf8ToBytes(rt.bytesToUtf8(rt.toStr(s)).toLowerCase()); });
    reg("mb_strtoupper", function (s) { return rt.utf8ToBytes(rt.bytesToUtf8(rt.toStr(s)).toUpperCase()); });
    reg("mb_strpos", function (h, n, off) {
        const hu = Array.from(rt.bytesToUtf8(rt.toStr(h))), nu = Array.from(rt.bytesToUtf8(rt.toStr(n)));
        let o = off !== undefined ? Number(rt.toInt(off)) : 0;
        if (o < 0) o = Math.max(0, hu.length + o);
        const hs = hu.slice(o).join(""), ns = nu.join("");
        const idx = hs.indexOf(ns);
        if (idx < 0) return false;
        return BigInt(o + Array.from(hs.slice(0, idx)).length);
    });

    // ---- Math ----
    reg("abs", function (v) {
        v = rt.deref(v);
        if (typeof v === "bigint") return v < 0n ? rt.clampInt64OrFloat(-v) : v;
        return Math.abs(rt.toFloat(v));
    });
    reg("floor", function (v) { return Math.floor(rt.toFloat(v)); });
    reg("ceil", function (v) { return Math.ceil(rt.toFloat(v)); });
    reg("round", function (v, prec) {
        const f = rt.toFloat(v);
        const p = prec === undefined ? 0 : Number(rt.toInt(prec));
        const mul = Math.pow(10, p);
        const scaled = f * mul;
        const r = scaled >= 0 ? Math.floor(scaled + 0.5) : Math.ceil(scaled - 0.5);
        return r / mul;
    });
    reg("pow", function (a, b) { return R.pow(a, b); });
    reg("sqrt", function (v) { return Math.sqrt(rt.toFloat(v)); });
    reg("intdiv", function (a, b) {
        const ia = rt.toInt(a), ib = rt.toInt(b);
        if (ib === 0n) rt.throwError("DivisionByZeroError", "Division by zero");
        if (ia === rt.I64_MIN && ib === -1n) rt.throwError("ArithmeticError", "Division of PHP_INT_MIN by -1 is not an integer");
        return ia / ib;
    });
    reg("fmod", function (a, b) { return rt.toFloat(a) % rt.toFloat(b); });
    reg("min", function () {
        const list = (arguments.length === 1 && arguments[0] && arguments[0].__php === 1)
            ? Array.from(arguments[0].m.values()).map(rt.deref) : Array.from(arguments).map(rt.deref);
        let best = list[0];
        for (let i = 1; i < list.length; i++) if (R.lt(list[i], best)) best = list[i];
        return best;
    });
    reg("max", function () {
        const list = (arguments.length === 1 && arguments[0] && arguments[0].__php === 1)
            ? Array.from(arguments[0].m.values()).map(rt.deref) : Array.from(arguments).map(rt.deref);
        let best = list[0];
        for (let i = 1; i < list.length; i++) if (R.gt(list[i], best)) best = list[i];
        return best;
    });
    reg("pi", function () { return Math.PI; });
    reg("sin", function (v) { return Math.sin(rt.toFloat(v)); });
    reg("cos", function (v) { return Math.cos(rt.toFloat(v)); });
    reg("tan", function (v) { return Math.tan(rt.toFloat(v)); });
    reg("log", function (v, base) { return base === undefined ? Math.log(rt.toFloat(v)) : Math.log(rt.toFloat(v)) / Math.log(rt.toFloat(base)); });
    reg("log10", function (v) { return Math.log10(rt.toFloat(v)); });
    reg("log2", function (v) { return Math.log2(rt.toFloat(v)); });
    reg("exp", function (v) { return Math.exp(rt.toFloat(v)); });
    reg("dechex", function (v) { return BigInt.asUintN(64, rt.toInt(v)).toString(16); });
    reg("hexdec", function (s) { return BigInt("0x" + (rt.toStr(s).replace(/[^0-9a-fA-F]/g, "") || "0")); });
    reg("decbin", function (v) { return BigInt.asUintN(64, rt.toInt(v)).toString(2); });
    reg("bindec", function (s) { return BigInt("0b" + (rt.toStr(s).replace(/[^01]/g, "") || "0")); });
    reg("decoct", function (v) { return BigInt.asUintN(64, rt.toInt(v)).toString(8); });
    reg("octdec", function (s) { return BigInt("0o" + (rt.toStr(s).replace(/[^0-7]/g, "") || "0")); });
    reg("rand", function (lo, hi) {
        if (arguments.length < 2) return BigInt(Math.floor(Math.random() * 2147483647));
        const a = rt.toInt(lo), b = rt.toInt(hi);
        const span = Number(b - a) + 1;
        return a + BigInt(Math.floor(Math.random() * span));
    });
    reg("mt_rand", function (lo, hi) { return rt.funcs.get("rand").builtin.apply(null, arguments); });
    reg("uniqid", function (pfx, more) {
        const now = Date.now();
        const hex = Math.floor(now / 1000).toString(16).padStart(8, "0") + ((now % 1000) * 1000 + Math.floor(Math.random() * 1000)).toString(16).padStart(5, "0");
        return (pfx !== undefined ? rt.toStr(pfx) : "") + hex + (more && rt.truthy(more) ? ("." + Math.floor(Math.random() * 1e8).toString().padStart(8, "0")) : "");
    });

    // ---- JSON (json_encode / json_decode) ----
    let lastJsonErr = 0n;
    let lastJsonMsg = "No error";

    function phpToJsonVal(v, flags) {
        v = rt.deref(v);
        if (v === null || v === undefined || typeof v === "boolean") return v;
        if (typeof v === "bigint") {
            if (v >= -9007199254740991n && v <= 9007199254740991n) return Number(v);
            return Number(v);
        }
        if (typeof v === "number") return Number.isFinite(v) ? v : 0;
        if (typeof v === "string") return rt.bytesToUtf8(v);
        if (v.__php === 1) {
            const forceObj = (flags & 16) !== 0;
            let isList = !forceObj;
            if (isList) {
                let i = 0n;
                for (const k of v.m.keys()) { if (k !== i) { isList = false; break; } i++; }
            }
            if (isList) {
                const out = [];
                for (const val of v.m.values()) out.push(phpToJsonVal(val, flags));
                return out;
            }
            const obj = {};
            for (const [k, val] of v.m) obj[rt.bytesToUtf8(String(k))] = phpToJsonVal(val, flags);
            return obj;
        }
        if (v.__php === 2) {
            if (v.enumVal !== undefined) return phpToJsonVal(v.enumVal, flags);
            const js = rt.findMethod(v.cls, "jsonserialize");
            if (js) return phpToJsonVal(rt.invokeMethod(v, js, [], null, v.cls, v.cls, false), flags);
            const obj = {};
            for (const k in v.props) {
                if (k.charCodeAt(0) !== 0) obj[rt.bytesToUtf8(k)] = phpToJsonVal(v.props[k], flags);
            }
            return obj;
        }
        return null;
    }

    function jsonValToPhp(v, assoc) {
        if (v === null || v === undefined) return null;
        if (typeof v === "boolean") return v;
        if (typeof v === "number") return Number.isInteger(v) && v >= -9007199254740991 && v <= 9007199254740991 ? BigInt(v) : v;
        if (typeof v === "string") return rt.utf8ToBytes(v);
        if (Array.isArray(v)) {
            const a = rt.makeArr();
            for (let i = 0; i < v.length; i++) rt.arrPushInPlace(a, jsonValToPhp(v[i], assoc));
            return a;
        }
        if (assoc) {
            const a = rt.makeArr();
            for (const [k, val] of Object.entries(v)) rt.arrSetInPlace(a, rt.utf8ToBytes(k), jsonValToPhp(val, assoc));
            return a;
        }
        const o = rt.allocInstance(rt.classes.get("stdclass"));
        for (const [k, val] of Object.entries(v)) o.props[rt.utf8ToBytes(k)] = jsonValToPhp(val, assoc);
        return o;
    }
    rt.jsonValToPhp = jsonValToPhp;

    reg("json_encode", function (val, flags) {
        const f = flags === undefined ? 0 : Number(rt.toInt(flags));
        try {
            const jsVal = phpToJsonVal(val, f);
            let s = (f & 128) !== 0 ? JSON.stringify(jsVal, null, 4) : JSON.stringify(jsVal);
            if ((f & 64) === 0) s = s.replace(/\//g, "\\/");
            if ((f & 256) === 0) {
                s = s.replace(/[\u0080-\uFFFF]/g, ch => "\\u" + ch.charCodeAt(0).toString(16).padStart(4, "0"));
            }
            lastJsonErr = 0n; lastJsonMsg = "No error";
            return rt.utf8ToBytes(s);
        } catch (e) {
            lastJsonErr = 4n; lastJsonMsg = e.message || "Encoding error";
            if ((f & 4194304) !== 0) rt.throwError("JsonException", lastJsonMsg);
            return false;
        }
    });
    reg("json_decode", function (json, assoc, depth, flags) {
        const f = flags === undefined ? 0 : Number(rt.toInt(flags));
        const asArr = assoc !== undefined && rt.truthy(assoc);
        try {
            const parsed = JSON.parse(rt.bytesToUtf8(rt.toStr(json)));
            lastJsonErr = 0n; lastJsonMsg = "No error";
            return jsonValToPhp(parsed, asArr);
        } catch (e) {
            lastJsonErr = 4n; lastJsonMsg = "Syntax error";
            if ((f & 4194304) !== 0) rt.throwError("JsonException", lastJsonMsg);
            return null;
        }
    });
    reg("json_last_error", function () { return lastJsonErr; });
    reg("json_last_error_msg", function () { return lastJsonMsg; });

    // ---- PCRE (preg_match / preg_match_all / preg_replace / preg_replace_callback / preg_split / preg_quote) ----
    const pcreCache = new Map();
    function compilePcre(pattern, extraFlags) {
        const p = rt.toStr(pattern);
        const cacheKey = p + "\0" + (extraFlags || "");
        const cached = pcreCache.get(cacheKey);
        if (cached) {
            cached.re.lastIndex = 0;
            return cached;
        }
        if (p.length < 2) rt.throwError("ValueError", "Empty or invalid PCRE pattern");
        const open = p[0];
        const closeMap = { "(": ")", "[": "]", "{": "}", "<": ">" };
        const close = closeMap[open] || open;
        let end = -1;
        for (let i = 1; i < p.length; i++) {
            if (p[i] === "\\") { i++; continue; }
            if (p[i] === close) { end = i; break; }
        }
        if (end < 0) rt.throwError("ValueError", "No ending delimiter '" + close + "' found");
        let body = p.slice(1, end);
        const modStr = p.slice(end + 1);
        let jsFlags = extraFlags || "";
        let isUnicode = false;
        if (modStr.includes("i") && !jsFlags.includes("i")) jsFlags += "i";
        if (modStr.includes("m") && !jsFlags.includes("m")) jsFlags += "m";
        if (modStr.includes("s") && !jsFlags.includes("s")) jsFlags += "s";
        if (modStr.includes("u")) { isUnicode = true; if (!jsFlags.includes("u")) jsFlags += "u"; }
        if (modStr.includes("x")) {
            let out = "", inClass = false;
            for (let i = 0; i < body.length; i++) {
                const c = body[i];
                if (c === "\\" && i + 1 < body.length) { out += c + body[++i]; continue; }
                if (c === "[") inClass = true;
                else if (c === "]") inClass = false;
                if (!inClass) {
                    if (c === "#") { while (i < body.length && body[i] !== "\n") i++; continue; }
                    if (c === " " || c === "\t" || c === "\r" || c === "\n") continue;
                }
                out += c;
            }
            body = out;
        }
        // Unescape non-JS regex delimiter (e.g. `\#`, `~`, `%`, `!`)
        if (close !== "/" && close !== "\\" && !"^$.*+?()[]{}|".includes(close)) {
            body = body.split("\\" + close).join(close);
        }
        // Handle PCRE-only syntax outside character classes:
        // - `(?P<name>...)` -> `(?<name>...)`, `(?P=name)` -> `\k<name>`
        // - `(?>...)` atomic groups -> `(?:...)`
        // - inline flag switches `(?i)`, `(?s)`, `(?m)`, `(?J)`, `(?U)` (lift global flags / strip)
        // - possessive quantifiers `++`, `*+`, `?+`, `{m,n}+` -> greedy `+`, `*`, `?`, `{m,n}` (required by Symfony RouteCompiler!)
        // - `\A` -> `^`, `\z` / `\Z` -> `$`, `\K` -> strip
        let norm = "", inCls = false;
        for (let i = 0; i < body.length; i++) {
            const c = body[i];
            if (c === "\\" && i + 1 < body.length) {
                const nx = body[i + 1];
                if (!inCls && nx === "A") { norm += "^"; i++; continue; }
                if (!inCls && (nx === "z" || nx === "Z")) { norm += "$"; i++; continue; }
                if (!inCls && nx === "K") { i++; continue; }
                // PCRE short Unicode property `\pL` / `\PL` -> JS `\p{L}` / `\P{L}` (+ `/u` flag)
                if ((nx === "p" || nx === "P") && i + 2 < body.length) {
                    const p2 = body[i + 2];
                    if (p2 === "{") {
                        if (!jsFlags.includes("u")) jsFlags += "u";
                        if (body[i + 3] === "^") {
                            norm += "\\" + (nx === "p" ? "P" : "p") + "{";
                            i += 3; continue;
                        }
                    } else if ((p2 >= "A" && p2 <= "Z") || (p2 >= "a" && p2 <= "z")) {
                        if (!jsFlags.includes("u")) jsFlags += "u";
                        norm += "\\" + nx + "{" + p2 + "}";
                        i += 2; continue;
                    }
                }
                // In JS `/u` mode, identity escapes of non-syntax punctuation (e.g. `\:`, `\_`, `\'`, `\"`, `\#`, `\%`, `\~`, `\!`, `\,`, `\=`, `\@`) throw SyntaxError
                if (isUnicode && ":_;'\"#%~!,=@`".includes(nx)) {
                    norm += nx; i++; continue;
                }
                norm += c + nx; i++; continue;
            }
            if (c === "[" && !inCls) { inCls = true; norm += c; continue; }
            if (c === "]" && inCls) { inCls = false; norm += c; continue; }
            if (!inCls) {
                if (c === "(" && body[i + 1] === "?") {
                    if (body[i + 2] === "P" && body[i + 3] === "<") {
                        norm += "(?<"; i += 3; continue;
                    }
                    if (body[i + 2] === "P" && body[i + 3] === "=") {
                        const closeP = body.indexOf(")", i + 4);
                        if (closeP > 0) { norm += "\\k<" + body.slice(i + 4, closeP) + ">"; i = closeP; continue; }
                    }
                    if (body[i + 2] === ">") {
                        norm += "(?:"; i += 2; continue;
                    }
                    // Inline modifier `(?imsxUJ-...)`
                    const modM = /^\(\?([imsxUJ\-]+)\)/.exec(body.slice(i));
                    if (modM) {
                        const fl = modM[1].split("-")[0];
                        if (fl.includes("i") && !jsFlags.includes("i")) jsFlags += "i";
                        if (fl.includes("m") && !jsFlags.includes("m")) jsFlags += "m";
                        if (fl.includes("s") && !jsFlags.includes("s")) jsFlags += "s";
                        i += modM[0].length - 1;
                        continue;
                    }
                    // Scoped modifier `(?i:...)` -> `(?:...)`
                    const scM = /^\(\?([imsxUJ\-]+):/.exec(body.slice(i));
                    if (scM) {
                        const fl = scM[1].split("-")[0];
                        if (fl.includes("i") && !jsFlags.includes("i")) jsFlags += "i";
                        if (fl.includes("m") && !jsFlags.includes("m")) jsFlags += "m";
                        if (fl.includes("s") && !jsFlags.includes("s")) jsFlags += "s";
                        norm += "(?:";
                        i += scM[0].length - 1;
                        continue;
                    }
                }
                // Strip possessive `+` after `+`, `*`, `?`, `}`
                if ((c === "+" || c === "*" || c === "?" || c === "}") && body[i + 1] === "+") {
                    norm += c; i++; continue;
                }
            }
            norm += c;
        }
        // Unroll PCRE subpattern recursion `(?-N)` / `(?R)` / `(?0)` to depth 4 (e.g. FastRoute balanced `{...}`).
        // Inner unrolled copies convert capturing groups to `(?:...)` so capture indices remain exact.
        function stripCaptures(s) {
            let out = "", ic = false;
            for (let i = 0; i < s.length; i++) {
                const ch = s[i];
                if (ch === "\\" && i + 1 < s.length) { out += ch + s[++i]; continue; }
                if (ch === "[" && !ic) { ic = true; out += ch; continue; }
                if (ch === "]" && ic) { ic = false; out += ch; continue; }
                if (!ic && ch === "(") {
                    if (s[i + 1] !== "?") { out += "(?:"; continue; }
                    if (s[i + 2] === "<" && s[i + 3] !== "=" && s[i + 3] !== "!") {
                        const gt = s.indexOf(">", i + 3);
                        if (gt > 0) { out += "(?:"; i = gt; continue; }
                    }
                }
                out += ch;
            }
            return out;
        }
        if (/\(\?(?:-\d+|R|0)\)/.test(norm)) {
            let changed = true, guard = 0;
            while (changed && guard++ < 8) {
                changed = false;
                const stack = [];
                const caps = [];
                let ic = false;
                for (let i = 0; i < norm.length; i++) {
                    const ch = norm[i];
                    if (ch === "\\" && i + 1 < norm.length) { i++; continue; }
                    if (ch === "[" && !ic) { ic = true; continue; }
                    if (ch === "]" && ic) { ic = false; continue; }
                    if (!ic && ch === "(") {
                        let isCap = false, openLen = 1;
                        if (norm[i + 1] !== "?") {
                            isCap = true;
                        } else if (norm[i + 2] === "<" && norm[i + 3] !== "=" && norm[i + 3] !== "!") {
                            const gt = norm.indexOf(">", i + 3);
                            if (gt > 0) { isCap = true; openLen = gt - i + 1; }
                        }
                        const rec = { start: i, end: -1, openLen: openLen, isCap: isCap };
                        if (isCap) caps.push(rec);
                        stack.push(rec);
                    } else if (!ic && ch === ")") {
                        if (stack.length > 0) stack.pop().end = i;
                    }
                }
                for (let ci = 0; ci < caps.length; ci++) {
                    const g = caps[ci];
                    if (g.end < 0) continue;
                    const inner = norm.slice(g.start + g.openLen, g.end);
                    const rm = /\(\?-(\d+)\)/.exec(inner);
                    if (rm && Number(rm[1]) === 1) {
                        const tmpl = stripCaptures(inner);
                        let unrolled = tmpl.split(rm[0]).join("(?!)");
                        for (let d = 0; d < 3; d++) {
                            unrolled = tmpl.split(rm[0]).join("(?:" + unrolled + ")");
                        }
                        norm = norm.slice(0, g.start + g.openLen) + unrolled + norm.slice(g.end);
                        changed = true;
                        break;
                    }
                }
                if (!changed && /\(\?(?:R|0)\)/.test(norm)) {
                    const tmpl = stripCaptures(norm);
                    let unrolled = tmpl.replace(/\(\?(?:R|0)\)/g, "(?!)");
                    for (let d = 0; d < 3; d++) {
                        unrolled = tmpl.replace(/\(\?(?:R|0)\)/g, "(?:" + unrolled + ")");
                    }
                    norm = norm.replace(/\(\?(?:R|0)\)/g, "(?:" + unrolled + ")");
                    changed = true;
                }
            }
        }
        // Handle PCRE `(?| ... )` branch reset groups (used by FastRoute GroupCountBased!)
        let branchRemap = null;
        if (norm.includes("(?|")) {
            const remap = [];
            let out = "";
            let physIdx = 1, logIdx = 1;
            let ic = false;
            const scanBranchCaps = (str) => {
                const list = [];
                let bic = false;
                for (let k = 0; k < str.length; k++) {
                    const ch = str[k];
                    if (ch === "\\" && k + 1 < str.length) { k++; continue; }
                    if (ch === "[" && !bic) { bic = true; continue; }
                    if (ch === "]" && bic) { bic = false; continue; }
                    if (!bic && ch === "(") {
                        if (str[k + 1] !== "?") {
                            list.push(physIdx++);
                        } else if (str[k + 2] === "<" && str[k + 3] !== "=" && str[k + 3] !== "!") {
                            list.push(physIdx++);
                        }
                    }
                }
                return list;
            };
            for (let i = 0; i < norm.length; i++) {
                const ch = norm[i];
                if (ch === "\\" && i + 1 < norm.length) { out += ch + norm[++i]; continue; }
                if (ch === "[" && !ic) { ic = true; out += ch; continue; }
                if (ch === "]" && ic) { ic = false; out += ch; continue; }
                if (!ic && ch === "(" && norm[i + 1] === "?" && norm[i + 2] === "|") {
                    // Find matching ')' and split top-level '|' branches
                    let depth = 1, j = i + 3, bic = false, cur = "";
                    const brs = [];
                    for (; j < norm.length; j++) {
                        const c2 = norm[j];
                        if (c2 === "\\" && j + 1 < norm.length) { cur += c2 + norm[++j]; continue; }
                        if (c2 === "[" && !bic) { bic = true; cur += c2; continue; }
                        if (c2 === "]" && bic) { bic = false; cur += c2; continue; }
                        if (!bic) {
                            if (c2 === "(") { depth++; cur += c2; continue; }
                            if (c2 === ")") {
                                depth--;
                                if (depth === 0) { brs.push(cur); break; }
                                cur += c2; continue;
                            }
                            if (c2 === "|" && depth === 1) { brs.push(cur); cur = ""; continue; }
                        }
                        cur += c2;
                    }
                    const brRec = { kind: "reset", baseLog: logIdx, branches: [] };
                    let maxCaps = 0;
                    out += "(?:";
                    for (let bi = 0; bi < brs.length; bi++) {
                        if (bi > 0) out += "|";
                        const sent = physIdx++;
                        const subCaps = scanBranchCaps(brs[bi]);
                        if (subCaps.length > maxCaps) maxCaps = subCaps.length;
                        brRec.branches.push({ sent: sent, caps: subCaps });
                        out += "(" + brs[bi] + ")";
                    }
                    out += ")";
                    logIdx += maxCaps;
                    remap.push(brRec);
                    i = j;
                    continue;
                }
                if (!ic && ch === "(") {
                    if (norm[i + 1] !== "?" || (norm[i + 2] === "<" && norm[i + 3] !== "=" && norm[i + 3] !== "!")) {
                        remap.push({ kind: "cap", phys: physIdx++, log: logIdx++ });
                    }
                }
                out += ch;
            }
            norm = out;
            branchRemap = remap;
        }
        // Handle PCRE `(*SKIP)(*F)` / `(*SKIP)(*FAIL)` top-level skip branches
        let hasSkip = false;
        if (norm.includes("(*SKIP)")) {
            const branches = [];
            let cur = "", depth = 0, ic = false;
            for (let i = 0; i < norm.length; i++) {
                const ch = norm[i];
                if (ch === "\\" && i + 1 < norm.length) { cur += ch + norm[++i]; continue; }
                if (ch === "[" && !ic) { ic = true; cur += ch; continue; }
                if (ch === "]" && ic) { ic = false; cur += ch; continue; }
                if (!ic) {
                    if (ch === "(") depth++;
                    else if (ch === ")" && depth > 0) depth--;
                    else if (ch === "|" && depth === 0) { branches.push(cur); cur = ""; continue; }
                }
                cur += ch;
            }
            branches.push(cur);
            const skipParts = [], keepParts = [];
            for (const br of branches) {
                if (/\(\*SKIP\)\s*\(\*F(?:AIL)?\)/.test(br)) {
                    skipParts.push(stripCaptures(br.replace(/\(\*SKIP\)\s*\(\*F(?:AIL)?\)/g, "")));
                } else {
                    keepParts.push(br);
                }
            }
            if (skipParts.length > 0 && keepParts.length > 0) {
                hasSkip = true;
                norm = "(" + skipParts.join("|") + ")|" + keepParts.join("|");
                if (!jsFlags.includes("g")) jsFlags += "g";
            }
        }
        if (modStr.includes("A") && !norm.startsWith("^")) norm = "^(?:" + norm + ")";
        // Count capturing groups & named groups so `preg_match_all` initializes every group even on 0 matches!
        let groupCount = 0;
        const groupNames = [];
        let gInCls = false;
        for (let i = 0; i < norm.length; i++) {
            const ch = norm[i];
            if (ch === "\\") { i++; continue; }
            if (ch === "[" && !gInCls) { gInCls = true; continue; }
            if (ch === "]" && gInCls) { gInCls = false; continue; }
            if (!gInCls && ch === "(") {
                if (norm[i + 1] === "?") {
                    if (norm[i + 2] === "<" && norm[i + 3] !== "=" && norm[i + 3] !== "!") {
                        const gt = norm.indexOf(">", i + 3);
                        if (gt > 0) groupNames.push(norm.slice(i + 3, gt));
                        groupCount++;
                    }
                } else {
                    groupCount++;
                }
            }
        }
        if (hasSkip && groupCount > 0) groupCount--;
        const entry = { re: new RegExp(norm, jsFlags), isUnicode: isUnicode, groupCount: groupCount, groupNames: groupNames, hasSkip: hasSkip, branchRemap: branchRemap };
        if (pcreCache.size > 512) pcreCache.clear();
        pcreCache.set(cacheKey, entry);
        return entry;
    }

    function shiftSkipMatch(m) {
        const out = [m[0]];
        for (let i = 2; i < m.length; i++) out.push(m[i]);
        out.index = m.index;
        out.groups = m.groups;
        return out;
    }

    function remapBranchReset(m, remap) {
        if (!remap) return m;
        const out = [m[0]];
        for (let i = 0; i < remap.length; i++) {
            const step = remap[i];
            if (step.kind === "cap") {
                out[step.log] = m[step.phys];
            } else {
                for (let bi = 0; bi < step.branches.length; bi++) {
                    const br = step.branches[bi];
                    if (m[br.sent] !== undefined) {
                        for (let ci = 0; ci < br.caps.length; ci++) {
                            out[step.baseLog + ci] = m[br.caps[ci]];
                        }
                        break;
                    }
                }
            }
        }
        out.index = m.index;
        out.groups = m.groups;
        return out;
    }

    function buildMatchArray(m, isUnicode, flags) {
        const arr = rt.makeArr();
        const capOff = (flags & 256) !== 0;
        const unmNull = (flags & 512) !== 0;
        const conv = s => s === undefined ? (unmNull ? null : "") : (isUnicode ? rt.utf8ToBytes(s) : s);
        let limit = m.length;
        if (!unmNull) {
            while (limit > 1 && m[limit - 1] === undefined) limit--;
        }
        for (let i = 0; i < limit; i++) {
            const val = conv(m[i]);
            if (capOff) {
                const pair = rt.makeArr();
                rt.arrPushInPlace(pair, val);
                rt.arrPushInPlace(pair, BigInt(m[i] === undefined ? -1 : m.index));
                rt.arrSetInPlace(arr, BigInt(i), pair);
            } else {
                rt.arrSetInPlace(arr, BigInt(i), val);
            }
        }
        if (m.groups) {
            for (const [k, v] of Object.entries(m.groups)) {
                if (!unmNull && v === undefined) continue;
                const val = conv(v);
                if (capOff) {
                    const pair = rt.makeArr();
                    rt.arrPushInPlace(pair, val);
                    rt.arrPushInPlace(pair, BigInt(v === undefined ? -1 : m.index));
                    rt.arrSetInPlace(arr, k, pair);
                } else {
                    rt.arrSetInPlace(arr, k, val);
                }
            }
        }
        return arr;
    }

    reg("preg_match", function (pat, subj, matchesRef, flags, offset) {
        const c = compilePcre(pat, "");
        const f = flags !== undefined ? Number(rt.toInt(flags)) : 0;
        let s = rt.toStr(subj);
        const off = offset !== undefined ? Number(rt.toInt(offset)) : 0;
        if (off > 0) s = s.slice(off);
        const text = c.isUnicode ? rt.bytesToUtf8(s) : s;
        let m;
        if (c.hasSkip) {
            c.re.lastIndex = 0;
            while ((m = c.re.exec(text)) !== null) {
                if (m[1] === undefined) { m = shiftSkipMatch(m); break; }
                if (m[0].length === 0) c.re.lastIndex++;
            }
        } else {
            m = c.re.exec(text);
        }
        if (!m) {
            if (matchesRef && rt.isRef(matchesRef)) matchesRef.v = rt.makeArr();
            return 0n;
        }
        if (c.branchRemap) m = remapBranchReset(m, c.branchRemap);
        if (matchesRef && rt.isRef(matchesRef)) matchesRef.v = buildMatchArray(m, c.isUnicode, f);
        return 1n;
    }, 4);

    reg("preg_match_all", function (pat, subj, matchesRef, flags, offset) {
        const c = compilePcre(pat, "g");
        const f = flags !== undefined ? Number(rt.toInt(flags)) : 1;
        let s = rt.toStr(subj);
        const off = offset !== undefined ? Number(rt.toInt(offset)) : 0;
        if (off > 0) s = s.slice(off);
        const text = c.isUnicode ? rt.bytesToUtf8(s) : s;
        const all = [];
        let m;
        while ((m = c.re.exec(text)) !== null) {
            if (c.hasSkip) {
                if (m[1] === undefined) {
                    let sm = shiftSkipMatch(m);
                    if (c.branchRemap) sm = remapBranchReset(sm, c.branchRemap);
                    all.push(sm);
                }
            } else {
                all.push(c.branchRemap ? remapBranchReset(m, c.branchRemap) : m);
            }
            if (m[0].length === 0) c.re.lastIndex++;
        }
        if (matchesRef && rt.isRef(matchesRef)) {
            const out = rt.makeArr();
            if ((f & 2) !== 0) { // PREG_SET_ORDER
                for (const item of all) rt.arrPushInPlace(out, buildMatchArray(item, c.isUnicode, f));
            } else { // PREG_PATTERN_ORDER
                const cols = all.length > 0 ? all[0].length : (c.groupCount + 1);
                for (let g = 0; g < cols; g++) {
                    const col = rt.makeArr();
                    for (const item of all) {
                        const v = item[g] === undefined ? "" : (c.isUnicode ? rt.utf8ToBytes(item[g]) : item[g]);
                        rt.arrPushInPlace(col, v);
                    }
                    rt.arrSetInPlace(out, BigInt(g), col);
                }
                for (let gi = 0; gi < c.groupNames.length; gi++) {
                    const gn = c.groupNames[gi];
                    const col = rt.makeArr();
                    for (const item of all) {
                        const raw = item.groups ? item.groups[gn] : undefined;
                        const v = raw === undefined ? "" : (c.isUnicode ? rt.utf8ToBytes(raw) : raw);
                        rt.arrPushInPlace(col, v);
                    }
                    rt.arrSetInPlace(out, gn, col);
                }
            }
            matchesRef.v = out;
        }
        return BigInt(all.length);
    }, 4);

    reg("preg_replace", function (pat, rep, subj, limit, countRef) {
        const lim = limit !== undefined ? Number(rt.toInt(limit)) : -1;
        const pats = pat && pat.__php === 1 ? Array.from(pat.m.values()) : [pat];
        const reps = rep && rep.__php === 1 ? Array.from(rep.m.values()) : pats.map(() => rep);
        let total = 0;
        function runOne(s) {
            let cur = rt.toStr(s);
            for (let i = 0; i < pats.length; i++) {
                const c = compilePcre(pats[i], "g");
                const r = rt.toStr(reps[i] !== undefined ? reps[i] : "");
                const text = c.isUnicode ? rt.bytesToUtf8(cur) : cur;
                const repTxt = c.isUnicode ? rt.bytesToUtf8(r) : r;
                let done = 0;
                const replaced = text.replace(c.re, function () {
                    if (c.hasSkip && arguments[1] !== undefined) return arguments[0];
                    if (lim >= 0 && done >= lim) return arguments[0];
                    done++; total++;
                    const args = arguments;
                    const shift = c.hasSkip ? 1 : 0;
                    return repTxt.replace(/\\(\d+)|\$(\d+)|\$\{(\d+)\}/g, function (_, a, b, d) {
                        const gi = Number(a || b || d);
                        const idx = gi === 0 ? 0 : gi + shift;
                        return args[idx] !== undefined ? args[idx] : "";
                    });
                });
                cur = c.isUnicode ? rt.utf8ToBytes(replaced) : replaced;
            }
            return cur;
        }
        if (subj && subj.__php === 1) {
            const out = rt.makeArr();
            for (const [k, v] of subj.m) rt.arrSetInPlace(out, k, runOne(v));
            if (countRef && rt.isRef(countRef)) countRef.v = BigInt(total);
            return out;
        }
        const res = runOne(subj);
        if (countRef && rt.isRef(countRef)) countRef.v = BigInt(total);
        return res;
    }, 16);

    reg("preg_replace_callback", function (pat, cb, subj, limit, countRef) {
        const lim = limit !== undefined ? Number(rt.toInt(limit)) : -1;
        const c = compilePcre(pat, "g");
        let total = 0;
        const text = c.isUnicode ? rt.bytesToUtf8(rt.toStr(subj)) : rt.toStr(subj);
        let done = 0;
        const res = text.replace(c.re, function () {
            if (c.hasSkip && arguments[1] !== undefined) return arguments[0];
            if (lim >= 0 && done >= lim) return arguments[0];
            done++; total++;
            const mArr = rt.makeArr();
            const argCount = arguments.length;
            // arguments: (match, p1, p2, ..., offset, string, [groups])
            const hasGroups = typeof arguments[argCount - 1] === "object" && arguments[argCount - 1] !== null;
            const groupEnd = hasGroups ? argCount - 3 : argCount - 2;
            let outIdx = 0;
            for (let i = 0; i < groupEnd; i++) {
                if (c.hasSkip && i === 1) continue;
                const v = arguments[i] === undefined ? "" : (c.isUnicode ? rt.utf8ToBytes(arguments[i]) : arguments[i]);
                rt.arrSetInPlace(mArr, BigInt(outIdx++), v);
            }
            if (hasGroups) {
                for (const [k, v] of Object.entries(arguments[argCount - 1])) {
                    rt.arrSetInPlace(mArr, k, v === undefined ? "" : (c.isUnicode ? rt.utf8ToBytes(v) : v));
                }
            }
            const r = rt.toStr(rt.call(cb, [mArr], null, null, false));
            return c.isUnicode ? rt.bytesToUtf8(r) : r;
        });
        if (countRef && rt.isRef(countRef)) countRef.v = BigInt(total);
        return c.isUnicode ? rt.utf8ToBytes(res) : res;
    }, 16);

    reg("preg_split", function (pat, subj, limit, flags) {
        const c = compilePcre(pat, "g");
        const lim = limit !== undefined ? Number(rt.toInt(limit)) : -1;
        const f = flags !== undefined ? Number(rt.toInt(flags)) : 0;
        const noEmpty = (f & 1) !== 0;
        const delimCap = (f & 2) !== 0;
        const offCap = (f & 4) !== 0;
        const s = c.isUnicode ? rt.bytesToUtf8(rt.toStr(subj)) : rt.toStr(subj);
        const out = rt.makeArr();
        const pushPart = (seg, off) => {
            const p = c.isUnicode ? rt.utf8ToBytes(seg) : seg;
            if (noEmpty && p.length === 0) return false;
            if (offCap) {
                const pair = rt.makeArr();
                rt.arrPushInPlace(pair, p);
                rt.arrPushInPlace(pair, BigInt(off));
                rt.arrPushInPlace(out, pair);
            } else {
                rt.arrPushInPlace(out, p);
            }
            return true;
        };
        let lastPos = 0, added = 0, m;
        c.re.lastIndex = 0;
        while ((m = c.re.exec(s)) !== null) {
            if (c.hasSkip && m[1] !== undefined) {
                if (m[0].length === 0) c.re.lastIndex++;
                continue;
            }
            if (lim > 0 && added === lim - 1) break;
            if (pushPart(s.slice(lastPos, m.index), lastPos)) added++;
            if (delimCap) {
                const startG = c.hasSkip ? 2 : 1;
                for (let g = startG; g < m.length; g++) {
                    if (m[g] !== undefined) pushPart(m[g], m.index);
                }
            }
            lastPos = m.index + m[0].length;
            if (m[0].length === 0) {
                c.re.lastIndex++;
                if (c.re.lastIndex > s.length) break;
            }
        }
        pushPart(s.slice(lastPos), lastPos);
        return out;
    });

    reg("preg_quote", function (s, delim) {
        s = rt.toStr(s).replace(/[.\\+*?\[\^\]$(){}=!<>|:\-#]/g, "\\$&");
        if (delim !== undefined) {
            const d = rt.toStr(delim)[0];
            if (d) s = s.split(d).join("\\" + d);
        }
        return s;
    });

    // ---- Date / Time ----
    let defaultTz = "UTC";
    function formatPhpDate(fmt, tsSec, utc) {
        const d = new Date(tsSec * 1000);
        const Y = utc ? d.getUTCFullYear() : d.getFullYear();
        const M = (utc ? d.getUTCMonth() : d.getMonth()) + 1;
        const D = utc ? d.getUTCDate() : d.getDate();
        const W = utc ? d.getUTCDay() : d.getDay();
        const H = utc ? d.getUTCHours() : d.getHours();
        const I = utc ? d.getUTCMinutes() : d.getMinutes();
        const S = utc ? d.getUTCSeconds() : d.getSeconds();
        const pad2 = n => String(n).padStart(2, "0");
        const days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        const daysFull = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
        const mons = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        const monsFull = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
        let out = "";
        for (let i = 0; i < fmt.length; i++) {
            const ch = fmt[i];
            if (ch === "\\" && i + 1 < fmt.length) { out += fmt[++i]; continue; }
            switch (ch) {
                case "Y": out += String(Y).padStart(4, "0"); break;
                case "y": out += String(Y).slice(-2); break;
                case "m": out += pad2(M); break;
                case "n": out += String(M); break;
                case "M": out += mons[M - 1]; break;
                case "F": out += monsFull[M - 1]; break;
                case "d": out += pad2(D); break;
                case "j": out += String(D); break;
                case "D": out += days[W]; break;
                case "l": out += daysFull[W]; break;
                case "w": out += String(W); break;
                case "N": out += String(W === 0 ? 7 : W); break;
                case "H": out += pad2(H); break;
                case "G": out += String(H); break;
                case "h": out += pad2((H % 12) || 12); break;
                case "g": out += String((H % 12) || 12); break;
                case "i": out += pad2(I); break;
                case "s": out += pad2(S); break;
                case "a": out += H < 12 ? "am" : "pm"; break;
                case "A": out += H < 12 ? "AM" : "PM"; break;
                case "U": out += String(Math.floor(tsSec)); break;
                case "c": out += formatPhpDate("Y-m-d\\TH:i:s+00:00", tsSec, true); break;
                case "r": out += formatPhpDate("D, d M Y H:i:s +0000", tsSec, true); break;
                case "P": out += "+00:00"; break;
                case "O": out += "+0000"; break;
                case "T": case "e": out += utc ? "UTC" : defaultTz; break;
                default: out += ch; break;
            }
        }
        return out;
    }
    const _hasPerf = typeof performance === "object" && performance !== null && typeof performance.now === "function";
    const _epochBaseMs = Date.now();
    const _perfBaseMs = _hasPerf ? performance.now() : 0;
    function nowHighResMs() {
        return _hasPerf ? (_epochBaseMs + (performance.now() - _perfBaseMs)) : Date.now();
    }
    reg("time", function () { return BigInt(Math.floor(Date.now() / 1000)); });
    reg("microtime", function (asFloat) {
        const ms = nowHighResMs();
        if (asFloat && rt.truthy(asFloat)) return ms / 1000.0;
        const sec = Math.floor(ms / 1000);
        const frac = ((ms - sec * 1000) / 1000).toFixed(8);
        return frac + " " + sec;
    });
    reg("hrtime", function (asNum) {
        const ms = _hasPerf ? performance.now() : Date.now();
        const ns = BigInt(Math.round(ms * 1000000));
        if (asNum && rt.truthy(asNum)) return ns;
        const a = rt.makeArr();
        const secs = Math.floor(ms / 1000);
        rt.arrPushInPlace(a, BigInt(secs));
        rt.arrPushInPlace(a, BigInt(Math.round((ms - secs * 1000) * 1000000)));
        return a;
    });
    reg("date", function (fmt, ts) {
        const t = (ts === undefined || rt.deref(ts) === null) ? Math.floor(Date.now() / 1000) : Number(rt.toInt(ts));
        return formatPhpDate(rt.toStr(fmt), t, defaultTz === "UTC");
    });
    reg("gmdate", function (fmt, ts) {
        const t = (ts === undefined || rt.deref(ts) === null) ? Math.floor(Date.now() / 1000) : Number(rt.toInt(ts));
        return formatPhpDate(rt.toStr(fmt), t, true);
    });
    reg("strtotime", function (s, base) {
        s = rt.toStr(s).trim();
        const b = base !== undefined ? Number(rt.toInt(base)) * 1000 : Date.now();
        if (s.toLowerCase() === "now") return BigInt(Math.floor(b / 1000));
        const rel = /^([+-]?\d+)\s*(second|minute|hour|day|week)s?$/i.exec(s);
        if (rel) {
            const mult = { second: 1, minute: 60, hour: 3600, day: 86400, week: 604800 }[rel[2].toLowerCase()];
            return BigInt(Math.floor(b / 1000) + Number(rel[1]) * mult);
        }
        const ms = Date.parse(s);
        return Number.isNaN(ms) ? false : BigInt(Math.floor(ms / 1000));
    });
    reg("date_default_timezone_set", function (tz) { defaultTz = rt.toStr(tz); return true; });
    reg("date_default_timezone_get", function () { return defaultTz; });

    function makeDateTimeClass(name, immutable) {
        return R.defClass({
            name: name, kind: "class",
            methods: [
                {
                    name: "__construct", vis: 0,
                    builtin: function (timeStr) {
                        const s = timeStr === undefined || rt.deref(timeStr) === null ? "now" : rt.toStr(timeStr);
                        if (s === "now" || s === "") this.__ts = Math.floor(Date.now() / 1000);
                        else {
                            const t = rt.funcs.get("strtotime").builtin(s);
                            this.__ts = t === false ? Math.floor(Date.now() / 1000) : Number(t);
                        }
                    }
                },
                { name: "format", vis: 0, builtin: function (fmt) { return formatPhpDate(rt.toStr(fmt), this.__ts, true); } },
                { name: "getTimestamp", vis: 0, builtin: function () { return BigInt(this.__ts); } },
                {
                    name: "modify", vis: 0,
                    builtin: function (mod) {
                        const nt = rt.funcs.get("strtotime").builtin(rt.toStr(mod), BigInt(this.__ts));
                        const ts = nt === false ? this.__ts : Number(nt);
                        if (immutable) {
                            const copy = rt.allocInstance(this.cls);
                            copy.__ts = ts;
                            return copy;
                        }
                        this.__ts = ts;
                        return this;
                    }
                }
            ]
        });
    }
    rt._makeDateTimeClass = makeDateTimeClass;

    // ---- Crypto & Passwords (backed by Rust native `__zipp_php_crypto`) ----
    function cryptoCall(op, a, b, c) {
        if (typeof __zipp_php_crypto === "function") return __zipp_php_crypto(op, a, b, c);
        rt.throwError("Error", "Crypto native unavailable");
    }
    reg("hash", function (algo, data, bin) {
        return cryptoCall("hash", rt.toStr(algo).toLowerCase(), rt.toStr(data), !!(bin && rt.truthy(bin)));
    });
    reg("hash_hmac", function (algo, data, key, bin) {
        return cryptoCall("hmac", rt.toStr(algo).toLowerCase(), rt.toStr(data), rt.toStr(key) + "\0" + (bin && rt.truthy(bin) ? "1" : "0"));
    });
    reg("md5", function (s, bin) { return cryptoCall("hash", "md5", rt.toStr(s), !!(bin && rt.truthy(bin))); });
    reg("sha1", function (s, bin) { return cryptoCall("hash", "sha1", rt.toStr(s), !!(bin && rt.truthy(bin))); });
    reg("crc32", function (s) { return cryptoCall("crc32", rt.toStr(s), "", false); });
    reg("hash_equals", function (a, b) {
        a = rt.toStr(a); b = rt.toStr(b);
        if (a.length !== b.length) return false;
        let diff = 0;
        for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
        return diff === 0;
    });
    reg("random_bytes", function (len) {
        const n = Number(rt.toInt(len));
        if (n < 1) rt.throwError("ValueError", "random_bytes(): Argument #1 ($length) must be greater than 0");
        return cryptoCall("random_bytes", n, "", false);
    });
    reg("random_int", function (min, max) {
        const lo = rt.toInt(min), hi = rt.toInt(max);
        if (lo > hi) rt.throwError("ValueError", "random_int(): Argument #1 ($min) must be less than or equal to argument #2 ($max)");
        if (lo === hi) return lo;
        const raw = cryptoCall("random_bytes", 8, "", false);
        let u = 0n;
        for (let i = 0; i < 8; i++) u = (u << 8n) | BigInt(raw.charCodeAt(i) & 0xFF);
        return lo + (u % (hi - lo + 1n));
    });
    reg("password_hash", function (pw, algo, opts) {
        const salt = cryptoCall("random_bytes", 16, "", false);
        const saltHex = rt.funcs.get("bin2hex").builtin(salt);
        const dk = cryptoCall("pbkdf2", rt.toStr(pw), saltHex, 60000);
        return "$zipp$sha256$60000$" + saltHex + "$" + dk;
    });
    reg("password_verify", function (pw, hash) {
        const parts = rt.toStr(hash).split("$");
        if (parts.length !== 6 || parts[1] !== "zipp") return false;
        const iter = Number(parts[3]) || 60000;
        const saltHex = parts[4];
        const expected = parts[5];
        const dk = cryptoCall("pbkdf2", rt.toStr(pw), saltHex, iter);
        return rt.funcs.get("hash_equals").builtin(expected, dk);
    });

    // ---- Output buffering builtins ----
    reg("ob_start", function (cb) {
        rt.obStack.push({ chunks: [], cb: cb !== undefined && rt.deref(cb) !== null ? rt.deref(cb) : null });
        return true;
    });
    reg("ob_get_level", function () { return BigInt(rt.obStack.length); });
    reg("ob_get_contents", function () {
        if (rt.obStack.length === 0) return false;
        return rt.obStack[rt.obStack.length - 1].chunks.join("");
    });
    reg("ob_get_length", function () {
        if (rt.obStack.length === 0) return false;
        return BigInt(rt.obStack[rt.obStack.length - 1].chunks.join("").length);
    });
    reg("ob_clean", function () {
        if (rt.obStack.length === 0) return false;
        rt.obStack[rt.obStack.length - 1].chunks.length = 0;
        return true;
    });
    reg("ob_end_clean", function () {
        if (rt.obStack.length === 0) return false;
        rt.obStack.pop();
        return true;
    });
    reg("ob_get_clean", function () {
        if (rt.obStack.length === 0) return false;
        const top = rt.obStack.pop();
        return top.chunks.join("");
    });
    reg("ob_flush", function () {
        if (rt.obStack.length === 0) return false;
        const top = rt.obStack[rt.obStack.length - 1];
        const text = top.chunks.join("");
        top.chunks.length = 0;
        if (rt.obStack.length > 1) rt.obStack[rt.obStack.length - 2].chunks.push(text);
        else rt.writeRaw(text);
        return true;
    });
    reg("ob_end_flush", function () {
        if (rt.obStack.length === 0) return false;
        const top = rt.obStack.pop();
        let text = top.chunks.join("");
        if (top.cb !== null) text = rt.toStr(rt.call(top.cb, [text, 9n], null, null, false));
        if (rt.obStack.length > 0) rt.obStack[rt.obStack.length - 1].chunks.push(text);
        else rt.writeRaw(text);
        return true;
    });

    // ---- Filesystem & Streams (backed by Rust native `__zipp_php_fs` + `php://` wrappers) ----
    let nextResId = 1;
    function fsCall(op, a, b, c) {
        if (typeof __zipp_php_fs === "function") return __zipp_php_fs(op, a, b, c);
        return null;
    }
    rt.fsCall = fsCall;

    reg("file_get_contents", function (path) {
        const p = rt.toStr(path);
        if (p === "php://input") return rt.rawInput || "";
        const res = fsCall("read", p, "", "");
        return res === null || res === undefined ? false : res;
    });
    reg("file_put_contents", function (path, data, flags) {
        const p = rt.toStr(path);
        const s = rt.toStr(data);
        const f = flags !== undefined ? Number(rt.toInt(flags)) : 0;
        const ok = fsCall((f & 8) !== 0 ? "append" : "write", p, s, "");
        return ok ? BigInt(s.length) : false;
    });
    reg("file_exists", function (p) { return !!fsCall("exists", rt.toStr(p), "", ""); });
    reg("is_file", function (p) { return !!fsCall("is_file", rt.toStr(p), "", ""); });
    reg("is_dir", function (p) { return !!fsCall("is_dir", rt.toStr(p), "", ""); });
    reg("is_readable", function (p) { return !!fsCall("exists", rt.toStr(p), "", ""); });
    reg("is_writable", function (p) { return !!fsCall("exists", rt.toStr(p), "", ""); });
    reg("filesize", function (p) { const r = fsCall("size", rt.toStr(p), "", ""); return r === null ? false : BigInt(r); });
    reg("filemtime", function (p) { const r = fsCall("mtime", rt.toStr(p), "", ""); return r === null ? false : BigInt(r); });
    reg("mkdir", function (p, mode, rec) { return !!fsCall("mkdir", rt.toStr(p), rec && rt.truthy(rec) ? "1" : "0", ""); });
    reg("rmdir", function (p) { return !!fsCall("rmdir", rt.toStr(p), "", ""); });
    reg("unlink", function (p) { return !!fsCall("unlink", rt.toStr(p), "", ""); });
    reg("rename", function (a, b) { return !!fsCall("rename", rt.toStr(a), rt.toStr(b), ""); });
    reg("copy", function (a, b) { return !!fsCall("copy", rt.toStr(a), rt.toStr(b), ""); });
    reg("realpath", function (p) { const r = fsCall("realpath", rt.toStr(p), "", ""); return r === null ? false : r; });
    reg("getcwd", function () { return fsCall("cwd", "", "", "") || "/"; });
    reg("sys_get_temp_dir", function () { return fsCall("tmpdir", "", "", "") || "/tmp"; });
    reg("scandir", function (p) {
        const list = fsCall("scandir", rt.toStr(p), "", "");
        if (!Array.isArray(list)) return false;
        const out = rt.makeArr();
        for (const item of list) rt.arrPushInPlace(out, item);
        return out;
    });
    reg("dirname", function (p, levels) {
        let s = rt.toStr(p).replace(/\\/g, "/");
        const n = levels === undefined ? 1 : Math.max(1, Number(rt.toInt(levels)));
        for (let i = 0; i < n; i++) {
            s = s.replace(/\/+$/, "");
            const idx = s.lastIndexOf("/");
            if (idx < 0) return ".";
            s = idx === 0 ? "/" : s.slice(0, idx);
        }
        return s;
    });
    reg("basename", function (p, suffix) {
        let s = rt.toStr(p).replace(/\\/g, "/").replace(/\/+$/, "");
        const idx = s.lastIndexOf("/");
        if (idx >= 0) s = s.slice(idx + 1);
        if (suffix !== undefined) {
            const suf = rt.toStr(suffix);
            if (suf && s.endsWith(suf)) s = s.slice(0, s.length - suf.length);
        }
        return s;
    });
    reg("pathinfo", function (p, flags) {
        const dir = rt.funcs.get("dirname").builtin(p);
        const base = rt.funcs.get("basename").builtin(p);
        const dot = base.lastIndexOf(".");
        const ext = dot > 0 ? base.slice(dot + 1) : "";
        const fname = dot > 0 ? base.slice(0, dot) : base;
        if (flags !== undefined) {
            const f = Number(rt.toInt(flags));
            if (f === 1) return dir;
            if (f === 2) return base;
            if (f === 4) return ext;
            if (f === 8) return fname;
        }
        const out = rt.makeArr();
        rt.arrSetInPlace(out, "dirname", dir);
        rt.arrSetInPlace(out, "basename", base);
        if (ext) rt.arrSetInPlace(out, "extension", ext);
        rt.arrSetInPlace(out, "filename", fname);
        return out;
    });

    // Stream handles (`fopen`/`fread`/`fwrite`/`fgets`/`feof`/`fseek`/`ftell`/`fclose`/`fstat`)
    defC("SEEK_SET", 0n);
    defC("SEEK_CUR", 1n);
    defC("SEEK_END", 2n);
    reg("fopen", function (path, mode) {
        const p = rt.toStr(path), m = rt.toStr(mode);
        if (p === "php://memory" || p === "php://temp" || p.startsWith("php://temp/")) {
            return { __php: 4, id: nextResId++, kind: "stream", path: p, mode: m, data: "", pos: 0, closed: false, mem: true };
        }
        if (p === "php://input") {
            return { __php: 4, id: nextResId++, kind: "stream", path: p, mode: m, data: rt.rawInput || "", pos: 0, closed: false, mem: true };
        }
        if (p === "php://output" || p === "php://stdout") {
            return { __php: 4, id: nextResId++, kind: "stream", path: p, mode: m, data: "", pos: 0, closed: false, stdout: true };
        }
        let data = "";
        if (m.startsWith("r")) {
            const r = fsCall("read", p, "", "");
            if (r === null || r === undefined) return false;
            data = r;
        } else if (m.startsWith("a")) {
            const r = fsCall("read", p, "", "");
            data = r || "";
        }
        return { __php: 4, id: nextResId++, kind: "stream", path: p, mode: m, data: data, pos: m.startsWith("a") ? data.length : 0, closed: false, dirty: m.startsWith("w") };
    });
    reg("fread", function (res, len) {
        res = rt.deref(res);
        if (!res || res.__php !== 4 || res.closed) return false;
        const n = Number(rt.toInt(len));
        const chunk = res.data.slice(res.pos, res.pos + n);
        res.pos += chunk.length;
        return chunk;
    });
    reg("fgets", function (res, len) {
        res = rt.deref(res);
        if (!res || res.__php !== 4 || res.closed || res.pos >= res.data.length) return false;
        const nl = res.data.indexOf("\n", res.pos);
        const end = nl >= 0 ? nl + 1 : res.data.length;
        const maxEnd = len !== undefined ? Math.min(end, res.pos + Math.max(0, Number(rt.toInt(len)) - 1)) : end;
        const line = res.data.slice(res.pos, maxEnd);
        res.pos = maxEnd;
        return line;
    });
    reg("fwrite", function (res, str, len) {
        res = rt.deref(res);
        if (!res || res.__php !== 4 || res.closed) return false;
        let s = rt.toStr(str);
        if (len !== undefined) s = s.slice(0, Number(rt.toInt(len)));
        if (res.stdout) { rt.echoStr(s); return BigInt(s.length); }
        res.data = res.data.slice(0, res.pos) + s + res.data.slice(res.pos + s.length);
        res.pos += s.length;
        res.dirty = true;
        return BigInt(s.length);
    });
    reg("fputs", function (r, s, l) { return rt.funcs.get("fwrite").builtin(r, s, l); });
    reg("feof", function (res) { res = rt.deref(res); return !res || res.closed || res.pos >= res.data.length; });
    reg("ftell", function (res) { res = rt.deref(res); return !res || res.closed ? false : BigInt(res.pos); });
    reg("rewind", function (res) { res = rt.deref(res); if (!res || res.closed) return false; res.pos = 0; return true; });
    reg("fseek", function (res, off, whence) {
        res = rt.deref(res);
        if (!res || res.closed) return -1n;
        const o = Number(rt.toInt(off));
        const w = whence === undefined ? 0 : Number(rt.toInt(whence));
        res.pos = Math.max(0, w === 1 ? res.pos + o : w === 2 ? res.data.length + o : o);
        return 0n;
    });
    reg("fclose", function (res) {
        res = rt.deref(res);
        if (!res || res.__php !== 4 || res.closed) return false;
        if (res.dirty && !res.mem && !res.stdout) fsCall("write", res.path, res.data, "");
        res.closed = true;
        return true;
    });
    reg("pclose", function (res) {
        res = rt.deref(res);
        if (!res || res.__php !== 4 || res.closed) return -1n;
        res.closed = true;
        return 0n;
    });
    reg("fstat", function (res) {
        res = rt.deref(res);
        if (!res || res.__php !== 4 || res.closed) return false;
        const sz = BigInt(res.data ? res.data.length : 0);
        const now = BigInt(Math.floor(Date.now() / 1000));
        const mode = 0o100644n;
        const a = rt.makeArr();
        const vals = [0n, 0n, mode, 1n, 0n, 0n, 0n, sz, now, now, now, -1n, -1n];
        const keys = ["dev", "ino", "mode", "nlink", "uid", "gid", "rdev", "size", "atime", "mtime", "ctime", "blksize", "blocks"];
        for (let i = 0; i < vals.length; i++) rt.arrSetInPlace(a, BigInt(i), vals[i]);
        for (let i = 0; i < keys.length; i++) rt.arrSetInPlace(a, keys[i], vals[i]);
        return a;
    });
    reg("get_resource_type", function (res) {
        res = rt.deref(res);
        if (!res || res.__php !== 4) rt.throwError("TypeError", "get_resource_type(): Argument #1 ($resource) must be of type resource");
        return res.closed ? "Unknown" : (res.kind || "stream");
    });
    reg("get_resource_id", function (res) {
        res = rt.deref(res);
        if (!res || res.__php !== 4) rt.throwError("TypeError", "get_resource_id(): Argument #1 ($resource) must be of type resource");
        return BigInt(res.id || 0);
    });
    reg("stream_copy_to_stream", function (from, to, maxLen, offset) {
        from = rt.deref(from); to = rt.deref(to);
        if (!from || from.__php !== 4 || from.closed || !to || to.__php !== 4 || to.closed) return false;
        if (offset !== undefined && Number(rt.toInt(offset)) >= 0) from.pos = Number(rt.toInt(offset));
        const max = (maxLen === undefined || rt.deref(maxLen) === null || Number(rt.toInt(maxLen)) < 0)
            ? from.data.length - from.pos : Number(rt.toInt(maxLen));
        const chunk = from.data.slice(from.pos, from.pos + max);
        from.pos += chunk.length;
        return rt.funcs.get("fwrite").builtin(to, chunk);
    });

    // ---- filter_var ----
    reg("filter_var", function (val, filter) {
        val = rt.deref(val);
        const f = filter === undefined ? 516 : Number(rt.toInt(filter));
        const s = rt.toStr(val);
        if (f === 274) return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(s) ? s : false; // EMAIL
        if (f === 273) return /^https?:\/\/[^\s/$.?#].[^\s]*$/i.test(s) ? s : false; // URL
        if (f === 257) { const p = rt.parseNumericString(s); return (p && p.isInt) ? p.iv : false; } // INT
        if (f === 259) { const p = rt.parseNumericString(s); return p ? p.fv : false; } // FLOAT
        if (f === 258) { // BOOL
            const lo = s.trim().toLowerCase();
            if (["1", "true", "on", "yes"].includes(lo) || val === true || val === 1n) return true;
            if (["0", "false", "off", "no", ""].includes(lo) || val === false || val === 0n) return false;
            return false;
        }
        if (f === 275) { // IPv4/IPv6 basic
            const parts = s.split(".");
            if (parts.length === 4 && parts.every(x => /^\d+$/.test(x) && Number(x) <= 255)) return s;
            if (s.includes(":") && /^[0-9a-fA-F:]+$/.test(s)) return s;
            return false;
        }
        return s;
    });
})(__zipp_php);
