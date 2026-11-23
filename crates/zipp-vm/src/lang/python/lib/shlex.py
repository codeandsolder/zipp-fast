"""POSIX shell-style lexical analyzer (`shlex`) for Zipp."""
import io
import re

__all__ = ["shlex", "split", "quote", "join"]

_SAFE_SHELL = re.compile(r"^[A-Za-z0-9_@%+=:,./-]+$")


def quote(s):
    if not s:
        return "''"
    if _SAFE_SHELL.match(s):
        return s
    return "'" + s.replace("'", "'\"'\"'") + "'"


def join(split_command):
    return " ".join(quote(arg) for arg in split_command)


class shlex:
    def __init__(self, instream=None, infile=None, posix=False, punctuation_chars=False):
        if isinstance(instream, str):
            instream = io.StringIO(instream)
        self.instream = instream
        self.infile = infile
        self.posix = bool(posix)
        self.eof = None if self.posix else ""
        self.commenters = "#"
        self.wordchars = (
            "abcdfeghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_"
        )
        if self.posix:
            self.wordchars += (
                "\u00df\u00e0\u00e1\u00e2\u00e3\u00e4\u00e5\u00e6\u00e7\u00e8\u00e9\u00ea\u00eb"
                "\u00ec\u00ed\u00ee\u00ef\u00f0\u00f1\u00f2\u00f3\u00f4\u00f5\u00f6\u00f8\u00f9"
                "\u00fa\u00fb\u00fc\u00fd\u00fe\u00ff\u00c0\u00c1\u00c2\u00c3\u00c4\u00c5\u00c6"
                "\u00c7\u00c8\u00c9\u00ca\u00cb\u00cc\u00cd\u00ce\u00cf\u00d0\u00d1\u00d2\u00d3"
                "\u00d4\u00d5\u00d6\u00d8\u00d9\u00da\u00db\u00dc\u00dd\u00de"
            )
        self.whitespace = " \t\r\n"
        self.whitespace_split = False
        self.quotes = "'\""
        self.escape = "\\"
        self.escapedquotes = '"'
        if not punctuation_chars:
            self.punctuation_chars = ""
        elif punctuation_chars is True:
            self.punctuation_chars = "();<>|&"
        else:
            self.punctuation_chars = str(punctuation_chars)
        if self.punctuation_chars:
            self._pushback_chars = []
            self.wordchars += "~-./*?="
            for ch in self.punctuation_chars:
                self.wordchars = self.wordchars.replace(ch, "")
        else:
            self._pushback_chars = []
        self.lineno = 1
        self.debug = 0
        self.token = ""
        self._pushback = []
        self._buf = instream.read() if instream is not None else ""
        self._pos = 0

    def push_token(self, tok):
        self._pushback.insert(0, tok)

    def _next_ch(self):
        if self._pushback_chars:
            return self._pushback_chars.pop()
        if self._pos >= len(self._buf):
            return ""
        ch = self._buf[self._pos]
        self._pos += 1
        if ch == "\n":
            self.lineno += 1
        return ch

    def _unread(self, ch):
        if ch:
            if ch == "\n":
                self.lineno -= 1
            self._pushback_chars.append(ch)

    def read_token(self):
        quoted = False
        escaped_state = " "
        state = " "
        parts = []
        while True:
            ch = self._next_ch()
            if state is None:
                break
            if state == " ":
                if not ch:
                    state = None
                    break
                if ch in self.whitespace:
                    if parts or (self.posix and quoted):
                        break
                    continue
                if ch in self.commenters:
                    while ch and ch != "\n":
                        ch = self._next_ch()
                    continue
                if self.posix and ch in self.escape:
                    escaped_state = "a"
                    state = ch
                    continue
                if ch in self.wordchars:
                    parts.append(ch)
                    state = "a"
                elif ch in self.punctuation_chars:
                    parts.append(ch)
                    state = "c"
                elif ch in self.quotes:
                    if not self.posix:
                        parts.append(ch)
                    state = ch
                elif self.whitespace_split:
                    parts.append(ch)
                    state = "a"
                else:
                    parts.append(ch)
                    if parts or (self.posix and quoted):
                        break
            elif state in self.quotes:
                quoted = True
                if not ch:
                    raise ValueError("No closing quotation")
                if ch == state:
                    if not self.posix:
                        parts.append(ch)
                        state = " "
                        break
                    state = "a"
                elif self.posix and ch in self.escape and state in self.escapedquotes:
                    escaped_state = state
                    state = ch
                else:
                    parts.append(ch)
            elif state in self.escape:
                if not ch:
                    raise ValueError("No escaped character")
                if escaped_state in self.quotes and ch != state and ch != escaped_state:
                    parts.append(state)
                parts.append(ch)
                state = escaped_state
            elif state in ("a", "c"):
                if not ch:
                    state = None
                    break
                if ch in self.whitespace:
                    state = " "
                    if parts or (self.posix and quoted):
                        break
                    continue
                if ch in self.commenters:
                    while ch and ch != "\n":
                        ch = self._next_ch()
                    if self.posix:
                        state = " "
                        if parts or quoted:
                            break
                    continue
                if state == "c":
                    if ch in self.punctuation_chars:
                        parts.append(ch)
                    else:
                        if ch not in self.whitespace:
                            self._unread(ch)
                        state = " "
                        break
                    continue
                if self.posix and ch in self.quotes:
                    state = ch
                elif self.posix and ch in self.escape:
                    escaped_state = "a"
                    state = ch
                elif ch in self.wordchars or ch in self.quotes or self.whitespace_split:
                    parts.append(ch)
                else:
                    if self.punctuation_chars:
                        self._unread(ch)
                    else:
                        self._pushback.insert(0, ch)
                    state = " "
                    if parts or (self.posix and quoted):
                        break
        result = "".join(parts)
        if self.posix and not quoted and result == "":
            return None
        return result

    def get_token(self):
        if self._pushback:
            return self._pushback.pop(0)
        raw = self.read_token()
        if raw is None:
            return self.eof
        return raw

    def __iter__(self):
        return self

    def __next__(self):
        tok = self.get_token()
        if tok == self.eof:
            raise StopIteration
        return tok


def split(s, comments=False, posix=True):
    if s is None:
        raise ValueError("s argument must not be None")
    lex = shlex(s, posix=posix)
    lex.whitespace_split = True
    if not comments:
        lex.commenters = ""
    return list(lex)
