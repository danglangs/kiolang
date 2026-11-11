#!/usr/bin/env python3
"""E4's table from eval/results/e4-sonnet-*.json: pass rates, tokens, cost, output tokens
per byte of final program, and where output tokens go (thinking, code, prose). The stream
reports thinking tokens but not thinking text; a reply's visible tokens are split between
its code and its prose by bytes. Writes eval/results/e4-summary.json.

  eval/e4_summary.py
"""
import json
import os

RES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "results")
RUNS = [("kio spec", "e4-sonnet-kio-spec.json"), ("kio cookbook", "e4-sonnet-kio-cookbook.json"),
        ("kio terse", "e4-sonnet-kio-terse.json"), ("python (E3)", "e4-sonnet-py.json")]
SAMPLE = ("01", "10", "20", "30", "40")


def first_call_input(t):
    """Input tokens of one first-round call, the same prompt shape in every run."""
    return sum(r["input_tokens"] for r in t if r["rounds_used"] == 1) / max(1, sum(1 for r in t if r["rounds_used"] == 1))


def split(rounds):
    think = code = prose = 0.0
    for rd in rounds:
        visible = rd["output_tokens"] - rd["thinking_tokens"]
        cb, pb = len((rd["program"] or "").encode()), len(rd["prose"].encode())
        think += rd["thinking_tokens"]
        code += visible * cb / max(1, cb + pb)
        prose += visible * pb / max(1, cb + pb)
    return think, code, prose


def main():
    out = {}
    for name, f in RUNS:
        path = os.path.join(RES, f)
        if not os.path.exists(path):
            continue
        d = json.load(open(path))
        t = d["tasks"]
        rounds = [rd for r in t for rd in r["rounds"]]
        final = sum(len((r["rounds"][-1]["program"] or "").encode()) for r in t if r["passed"])
        out_passed = sum(r["output_tokens"] for r in t if r["passed"])
        think, code, prose = split(rounds)
        tot = sum(r["output_tokens"] for r in t)
        one = [r for r in t if r["rounds_used"] == 1 and r["passed"]]
        out[name] = {
            "file": f, "system_bytes": d.get("system_bytes"), "tasks": len(t),
            "errors": [r["task"] for r in t if r.get("error")],
            "pass_first": sum(1 for r in t if r["pass_first"]), "pass_by_5": sum(1 for r in t if r["passed"]),
            "rounds": len(rounds),
            "output_tokens": tot, "thinking_tokens": round(think), "code_tokens": round(code),
            "prose_tokens": round(prose), "input_tokens": sum(r["input_tokens"] for r in t),
            "cost_usd": round(sum(r["cost_usd"] for r in t), 2),
            "out_per_final_byte": round(out_passed / max(1, final), 2),
            "out_per_byte_first_try": round(sum(r["output_tokens"] for r in one)
                                            / max(1, sum(len(r["rounds"][0]["program"].encode()) for r in one)), 2),
            "first_call_input_tokens": round(first_call_input(t)),
            "sample": [{"task": r["task"], "output_tokens": r["rounds"][0]["output_tokens"],
                        "thinking_tokens": r["rounds"][0]["thinking_tokens"],
                        "program_bytes": len((r["rounds"][0]["program"] or "").encode()),
                        "prose": r["rounds"][0]["prose"][:300]}
                       for r in t if r["task"][:2] in SAMPLE and r["rounds"]],
        }
    with open(os.path.join(RES, "e4-summary.json"), "w") as f:
        json.dump(out, f, indent=1)
    head = "%-13s %6s %6s %8s %8s %6s %6s %9s %7s %7s %7s" % (
        "prompt", "pass1", "pass5", "out tok", "think", "code", "prose", "in tok", "cost", "tok/B", "tok/B1")
    print(head)
    for name, s in out.items():
        print("%-13s %6d %6d %8d %8d %6d %6d %9d %7.2f %7.2f %7.2f" % (
            name, s["pass_first"], s["pass_by_5"], s["output_tokens"], s["thinking_tokens"], s["code_tokens"],
            s["prose_tokens"], s["input_tokens"], s["cost_usd"], s["out_per_final_byte"], s["out_per_byte_first_try"]))
    for name, s in out.items():
        print("\n" + name, "first-call input tokens", s["first_call_input_tokens"], "errors", s["errors"])
        for x in s["sample"]:
            print("  %s out %d think %d program %dB prose %r" % (x["task"], x["output_tokens"], x["thinking_tokens"],
                                                                x["program_bytes"], x["prose"][:120]))


if __name__ == "__main__":
    main()
