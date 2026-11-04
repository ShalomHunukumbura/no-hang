import httpx
import requests

# requests.get(url) in a comment is not a call.
r = requests.get("https://example.com", timeout=10)
h = httpx.get("https://example.com")
