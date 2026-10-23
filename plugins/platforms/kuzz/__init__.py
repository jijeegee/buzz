"""Kuzz platform plugin for Hermes Agent. The adapter loads only when registered."""


def register(ctx):
    from .adapter import register as _register

    _register(ctx)


__all__ = ["register"]
