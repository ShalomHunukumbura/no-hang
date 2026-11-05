import subprocess

import requests

subprocess.run(["make", "release"])  # no-hang: ignore[NH003] a release build takes what it takes
# no-hang: ignore
requests.get("https://status.example")
requests.post("https://hooks.example")  # no-hang: ignore[NH003] the wrong rule, so it's reported
