import requests

BASE = "https://api.example"


class Api:
    def __init__(self):
        self.session = requests.Session()
        self.session.timeout = 10  # does nothing: requests ignores it

    def fetch(self, path):
        return self.session.get(BASE + path)

    def fetch_bounded(self, path):
        return self.session.get(BASE + path, timeout=10)
