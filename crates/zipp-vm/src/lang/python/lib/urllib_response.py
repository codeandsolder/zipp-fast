"""Response wrappers for urllib (`urllib.response`)."""


class addbase:
    def __init__(self, fp):
        self.fp = fp

    def read(self, amt=None):
        return self.fp.read(amt)

    def readline(self, limit=-1):
        return self.fp.readline(limit)

    def readlines(self):
        return self.fp.readlines()

    def close(self):
        if self.fp is not None:
            self.fp.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
        return False

    def __iter__(self):
        return iter(self.fp)


class addclosehook(addbase):
    def __init__(self, fp, closehook, *hookargs):
        super().__init__(fp)
        self.closehook = closehook
        self.hookargs = hookargs

    def close(self):
        try:
            if self.closehook is not None:
                self.closehook(*self.hookargs)
                self.closehook = None
        finally:
            super().close()


class addinfo(addbase):
    def __init__(self, fp, headers):
        super().__init__(fp)
        self.headers = headers

    def info(self):
        return self.headers


class addinfourl(addinfo):
    def __init__(self, fp, headers, url, code=None):
        super().__init__(fp, headers)
        self.url = url
        self.code = code

    @property
    def status(self):
        return self.code

    def getcode(self):
        return self.code

    def geturl(self):
        return self.url
