#!/usr/bin/env python3
"""Sample memory, CPU and wake-ups for processes and their descendants on macOS."""
import argparse
import ctypes
import json
import subprocess
import time

RUSAGE_INFO_V2 = 2


class RusageInfoV2(ctypes.Structure):
    _fields_ = [
        ("uuid", ctypes.c_uint8 * 16),
        ("user_time", ctypes.c_uint64),
        ("system_time", ctypes.c_uint64),
        ("package_idle_wakeups", ctypes.c_uint64),
        ("interrupt_wakeups", ctypes.c_uint64),
        ("page_ins", ctypes.c_uint64),
        ("wired_size", ctypes.c_uint64),
        ("resident_size", ctypes.c_uint64),
        ("phys_footprint", ctypes.c_uint64),
        ("padding", ctypes.c_uint64 * 32),
    ]


class TimebaseInfo(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]


libc = ctypes.CDLL("libSystem.dylib")
timebase = TimebaseInfo()
libc.mach_timebase_info(ctypes.byref(timebase))


def descendants(roots):
    table = subprocess.check_output(["ps", "-axo", "pid=,ppid="], text=True)
    children = {}
    for row in table.splitlines():
        pid, parent = map(int, row.split())
        children.setdefault(parent, []).append(pid)
    found, pending = set(), list(roots)
    while pending:
        pid = pending.pop()
        found.add(pid)
        pending.extend(children.get(pid, []))
    return found


def totals(roots):
    usage = RusageInfoV2()
    count = footprint = cpu = wakeups = 0
    for pid in descendants(roots):
        if libc.proc_pid_rusage(pid, RUSAGE_INFO_V2, ctypes.byref(usage)) != 0:
            continue
        count += 1
        footprint += usage.phys_footprint
        cpu += usage.user_time + usage.system_time
        wakeups += usage.package_idle_wakeups + usage.interrupt_wakeups
    seconds = cpu * timebase.numer / timebase.denom / 1e9
    return count, footprint, seconds, wakeups


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pids", type=int, nargs="+")
    parser.add_argument("--seconds", type=float, default=30)
    parser.add_argument("--label", default="")
    args = parser.parse_args()

    _, _, start_cpu, start_wakeups = totals(args.pids)
    peak = 0
    end = time.monotonic() + args.seconds
    while time.monotonic() < end:
        processes, footprint, cpu, wakeups = totals(args.pids)
        peak = max(peak, footprint)
        time.sleep(1)
    processes, footprint, cpu, wakeups = totals(args.pids)
    print(json.dumps({
        "label": args.label,
        "processes": processes,
        "footprint_mb": round(footprint / 2**20),
        "peak_footprint_mb": round(max(peak, footprint) / 2**20),
        "cpu_percent": round((cpu - start_cpu) / args.seconds * 100, 1),
        "wakeups_per_second": round((wakeups - start_wakeups) / args.seconds, 1),
    }))


if __name__ == "__main__":
    main()
