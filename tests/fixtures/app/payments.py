"""Calls a payment gateway. Mentions requests.get( in a docstring, which isn't a call."""

import subprocess

import requests
from requests import post

GATEWAY = "https://gateway.example/pay"


def charge(order):
    return requests.post(GATEWAY, json=order.body)


def refund(order, timeout=None):
    return post(GATEWAY + "/refund", json=order.body, timeout=timeout)


def status(order):
    return requests.get(GATEWAY + "/status", params={"id": order.id}, timeout=(3.05, 30))


def receipt(order):
    return requests.get(GATEWAY + "/receipt", timeout=None)


def forward(**options):
    return requests.post(GATEWAY, **options)


def backup():
    subprocess.run(["pg_dump", "-f", "backup.sql"], check=True)
