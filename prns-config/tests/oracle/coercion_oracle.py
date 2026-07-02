import sys
import json

BOOLS = {
    "yes": True,
    "no": False,
    "on": True,
    "off": False,
    "1": True,
    "0": False,
    "true": True,
    "false": False,
}


def coerce(text):
    result = {}
    try:
        result["int"] = str(int(text))
    except Exception:
        result["int"] = None
    try:
        result["float"] = repr(float(text))
    except Exception:
        result["float"] = None
    result["bool"] = BOOLS.get(text.lower())
    return result


def main():
    items = json.load(sys.stdin)
    json.dump([coerce(text) for text in items], sys.stdout)


if __name__ == "__main__":
    main()
