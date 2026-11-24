#!/usr/bin/env python3
"""Writes the files the comparison cats: color.txt (135 MB, 24-bit colour), plain.txt (100 MB), unicode.txt (50 MB).
usage: gen_data.py <dir>"""
import os, random, sys

d = sys.argv[1]
os.makedirs(d, exist_ok=True)
rnd = random.Random(7)
words = "the quick brown fox jumps over a lazy dog while terminals render text at speed and cargo builds crates".split()

def sentence(n):
    return " ".join(rnd.choice(words) for _ in range(n))

def write(name, size, line):
    path = os.path.join(d, name)
    pool = [line() for _ in range(4096)]               # a pool of distinct lines, repeated to reach the size
    with open(path, "w") as f:
        n = 0
        while n < size:
            for l in pool:
                f.write(l); n += len(l.encode())
                if n >= size: break
    print(name, os.path.getsize(path) // 1_000_000, "MB")

def coloured():
    return "".join(f"\x1b[38;2;{rnd.randrange(256)};{rnd.randrange(256)};{rnd.randrange(256)}m{sentence(3)}\x1b[0m " for _ in range(4)) + "\n"

emoji = "😀🚀🔥🌈✨🎉🍎🐍🦀"
cjk = "終端機速度測試日本語のテキスト한국어텍스트"
def unicode_line():
    return " ".join(rnd.choice([sentence(2), "".join(rnd.choice(cjk) for _ in range(6)), "".join(rnd.choice(emoji) for _ in range(3)), "café naïve façade"]) for _ in range(8)) + "\n"

write("color.txt", 135_000_000, coloured)
write("plain.txt", 100_000_000, lambda: sentence(14) + "\n")
write("unicode.txt", 50_000_000, unicode_line)
