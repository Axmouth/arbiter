#!/usr/bin/env python3
"""Arbiter Python runner runtime (Layer B).

Vendored, stdlib-only. The worker writes this file once (reused across runs) and
invokes it per run with the handshake on argv; it imports the user's module, runs
the entrypoint, marshals the return value, captures errors, and writes a result
document. User code (Layer C) only implements `run(ctx)` (and optionally
`prepare(ctx)`) and never touches the wire format -- swapping the transport
(file -> socket) is a Layer B/A change.

Handshake (argv): --module M --entry E (default "run") --result-file PATH
--run-id ID --transport file --protocol N. The process env is reserved for the
user's own variables (e.g. PYTHONPATH); arbiter does not inject control vars.
"""

import dataclasses
import importlib
import inspect
import json
import sys
import traceback

PROTOCOL_VERSION = 1


def _parse_args(argv):
    args = {}
    i = 0
    while i < len(argv):
        token = argv[i]
        if token.startswith("--"):
            key = token[2:]
            value = argv[i + 1] if i + 1 < len(argv) else ""
            args[key] = value
            i += 2
        else:
            i += 1
    return args


class _Logger:
    """Routes structured logs to stderr (captured by the worker as run logs)."""

    def _emit(self, level, msg):
        sys.stderr.write("[{}] {}\n".format(level, msg))
        sys.stderr.flush()

    def debug(self, msg):
        self._emit("debug", msg)

    def info(self, msg):
        self._emit("info", msg)

    def warning(self, msg):
        self._emit("warning", msg)

    def error(self, msg):
        self._emit("error", msg)


class _State:
    """Scratch namespace; prepare() can stash warmed resources here for run()."""


class Context:
    def __init__(self, args):
        self.log = _Logger()
        self.state = _State()
        self.run_id = args.get("run-id")
        self.job_id = args.get("job-id")
        self.params = {}

    def progress(self, pct):
        # v1: no event stream consumed yet; surface progress as a log line.
        self.log.info("progress {}".format(pct))


def _jsonable(value):
    """Best-effort conversion of a return value to a json-serializable shape."""
    if value is None:
        return None
    model_dump = getattr(value, "model_dump", None)  # pydantic
    if callable(model_dump):
        try:
            return model_dump()
        except Exception:
            pass
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        return dataclasses.asdict(value)
    try:
        json.dumps(value)
        return value
    except (TypeError, ValueError):
        pass
    if hasattr(value, "__dict__"):
        return {k: v for k, v in vars(value).items() if not k.startswith("_")}
    return str(value)


def _as_callable(target):
    """Resolve the entrypoint to a callable taking ctx.

    Supports a plain function, or a class exposing run()/__call__ (the class is
    instantiated with no args first).
    """
    if inspect.isclass(target):
        instance = target()
        run = getattr(instance, "run", None)
        if callable(run):
            return run
        if callable(instance):
            return instance
        raise TypeError("class entrypoint has no callable run() or __call__")
    if callable(target):
        return target
    raise TypeError("entrypoint is not callable")


def _call(fn, ctx):
    """Call the entrypoint with ctx, tolerating zero-arg entrypoints."""
    try:
        return fn(ctx)
    except TypeError:
        try:
            sig = inspect.signature(fn)
        except (TypeError, ValueError):
            raise
        if not sig.parameters:
            return fn()
        raise


def main():
    args = _parse_args(sys.argv[1:])
    result_file = args.get("result-file")
    module_name = args.get("module")
    entry = args.get("entry") or "run"

    ctx = Context(args)
    status, output, error = "success", None, None
    try:
        if not module_name:
            raise RuntimeError("--module is not set")
        module = importlib.import_module(module_name)
        prepare = getattr(module, "prepare", None)
        if callable(prepare):
            _call(prepare, ctx)
        fn = _as_callable(getattr(module, entry))
        output = _jsonable(_call(fn, ctx))
    except Exception as exc:  # noqa: BLE001 - report any user error structurally
        status = "failed"
        error = {
            "type": type(exc).__name__,
            "message": str(exc),
            "stack": traceback.format_exception(type(exc), exc, exc.__traceback__),
        }

    doc = {
        "protocolVersion": PROTOCOL_VERSION,
        "status": status,
        "output": output,
        "error": error,
    }
    if result_file:
        with open(result_file, "w") as handle:
            json.dump(doc, handle)

    sys.exit(0 if status == "success" else 1)


if __name__ == "__main__":
    main()
