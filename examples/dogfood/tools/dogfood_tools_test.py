"""Aggregate dogfood tool tests for the documented *_test.py discovery pattern."""

from test_configure_forgejo import ConfigureForgejoTests  # noqa: F401
from test_interaction_profile import DogfoodInteractionProfileTests  # noqa: F401
from test_label_intake import LabelIntakeTests  # noqa: F401
from test_parse_secrets import ParseSecretsTests  # noqa: F401
