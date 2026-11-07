#!/usr/bin/env python3
"""Append fake CRI-format container logs to a file at a fixed rate."""
import argparse
import datetime
import json
import random
import time

LEVELS = ["DEBUG", "INFO", "INFO", "INFO", "WARN", "ERROR"]
ROUTES = ["/healthz", "/api/orders", "/api/users", "/api/cart", "/metrics"]


def line(seq):
    now = datetime.datetime.now(datetime.timezone.utc)
    body = {
        "seq": seq,
        "level": random.choice(LEVELS),
        "route": random.choice(ROUTES),
        "status": random.choice([200, 200, 200, 201, 404, 500]),
        "ms": random.randint(1, 900),
    }
    stamp = now.strftime("%Y-%m-%dT%H:%M:%S.%fZ")
    return f"{stamp} stdout F {json.dumps(body)}\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("path")
    parser.add_argument("--rate", type=int, default=200, help="lines per second")
    parser.add_argument("--seconds", type=float, default=0, help="0 runs forever")
    args = parser.parse_args()

    start = time.monotonic()
    seq = 0
    with open(args.path, "a", buffering=1) as out:
        while not args.seconds or time.monotonic() - start < args.seconds:
            target = int((time.monotonic() - start) * args.rate)
            while seq < target:
                out.write(line(seq))
                seq += 1
            time.sleep(0.01)


if __name__ == "__main__":
    main()
