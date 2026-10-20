#!/usr/bin/env python3
"""Tests for tools/gen_config.py. Standard library only, run by make."""

import contextlib
import importlib.util
import io
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location(
    "gen_config", HERE / "gen_config.py"
)
gen_config = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gen_config)

VALID = """\
# a comment
tuning:
  gc_ratio: 0.1
  gc_all: 1.0
  gc_threshold: 0.2

geometry:
  ramdisk:
    bus_bits: 2
    chip_bits: 2
    page_bits: 7
    block_bits: 19
    page_size: 8192
  zone:
    bus_bits: 3
    chip_bits: 3
    page_bits: 5
    block_bits: 21
    page_size: 8192
  bluedbm:
    bus_bits: 3
    chip_bits: 3
    page_bits: 7
    block_bits: 19
    page_size: 8192
  raspberry:
    bus_bits: 1
    chip_bits: 1
    page_bits: 4
    block_bits: 24
    page_size: 2048
  default:
    bus_bits: 3
    chip_bits: 3
    page_bits: 7
    block_bits: 19
    page_size: 8192
"""


def load(text):
    data, where = gen_config.parse(text)
    gen_config.validate(data, where)
    return data


def error(text):
    try:
        load(text)
    except gen_config.ConfigError as caught:
        return str(caught)
    raise AssertionError("expected a ConfigError")


class GrammarTests(unittest.TestCase):
    def test_tab_is_rejected(self):
        message = error("tuning:\n\tgc_ratio: 0.1\n")
        self.assertIn("ftl.yaml:2:", message)
        self.assertIn("tab", message)

    def test_flow_map_is_rejected(self):
        message = error("tuning: {gc_ratio: 0.1}\n")
        self.assertIn("ftl.yaml:1:", message)
        self.assertIn("unsupported", message)

    def test_quoted_string_is_rejected(self):
        message = error('tuning:\n  gc_ratio: "0.1"\n')
        self.assertIn("ftl.yaml:2:", message)
        self.assertIn("unsupported", message)

    def test_anchor_is_rejected(self):
        message = error("tuning:\n  gc_ratio: &ratio 0.1\n")
        self.assertIn("ftl.yaml:2:", message)
        self.assertIn("unsupported", message)

    def test_missing_key_is_rejected(self):
        message = error(VALID.replace("  gc_all: 1.0\n", ""))
        self.assertIn("missing key 'gc_all'", message)
        self.assertIn("ftl.yaml:", message)

    def test_unknown_key_is_rejected(self):
        altered = VALID.replace(
            "  gc_all: 1.0\n", "  gc_all: 1.0\n  gc_extra: 0.5\n"
        )
        message = error(altered)
        self.assertIn("ftl.yaml:5:", message)
        self.assertIn("unknown key 'gc_extra'", message)

    def test_duplicate_key_is_rejected(self):
        altered = VALID.replace(
            "  gc_all: 1.0\n", "  gc_all: 1.0\n  gc_all: 0.5\n"
        )
        message = error(altered)
        self.assertIn("ftl.yaml:5:", message)
        self.assertIn("duplicate key 'gc_all'", message)

    def test_odd_indentation_is_rejected(self):
        message = error("tuning:\n   gc_ratio: 0.1\n")
        self.assertIn("ftl.yaml:2:", message)
        self.assertIn("indentation", message)

    def test_indentation_must_step_one_level(self):
        message = error("tuning:\n    gc_ratio: 0.1\n")
        self.assertIn("ftl.yaml:2:", message)
        self.assertIn("one more indentation level", message)

    def test_flow_sequence_at_line_start_is_rejected(self):
        message = error("tuning:\n  [gc_ratio, 0.1]\n")
        self.assertIn("ftl.yaml:2:", message)
        self.assertIn("unsupported", message)

    def test_invalid_section_key_is_rejected(self):
        message = error("bad-key:\n  value: 1\n")
        self.assertIn("ftl.yaml:1:", message)
        self.assertIn("invalid key", message)

    def test_invalid_scalar_key_is_rejected(self):
        message = error("bad-key: 1\n")
        self.assertIn("ftl.yaml:1:", message)
        self.assertIn("invalid key", message)

    def test_line_without_a_colon_is_rejected(self):
        message = error("tuning\n")
        self.assertIn("ftl.yaml:1:", message)
        self.assertIn("expected 'key: value'", message)

    def test_missing_value_is_rejected(self):
        message = error("tuning:\n  gc_ratio:   \n")
        self.assertIn("ftl.yaml:2:", message)
        self.assertIn("missing value", message)

    def test_bare_word_value_is_rejected(self):
        message = error("tuning:\n  gc_ratio: lots\n")
        self.assertIn("ftl.yaml:2:", message)
        self.assertIn("not an integer or plain decimal", message)


class ValidationTests(unittest.TestCase):
    def test_page_size_must_be_a_power_of_two(self):
        message = error(VALID.replace("page_size: 8192", "page_size: 3000", 1))
        self.assertIn("ftl.yaml:", message)
        self.assertIn("power of two", message)

    def test_bit_count_must_be_at_least_one(self):
        message = error(VALID.replace("bus_bits: 2", "bus_bits: 0", 1))
        self.assertIn("1..=31", message)

    def test_bit_count_must_fit_the_address(self):
        message = error(VALID.replace("block_bits: 24", "block_bits: 31", 1))
        self.assertIn("uint32", message)

    def test_tuning_must_be_above_zero(self):
        message = error(VALID.replace("gc_ratio: 0.1", "gc_ratio: 0.0", 1))
        self.assertIn("(0.0, 1.0]", message)

    def test_tuning_must_not_exceed_one(self):
        message = error(VALID.replace("gc_ratio: 0.1", "gc_ratio: 1.1", 1))
        self.assertIn("(0.0, 1.0]", message)

    def test_decimal_bit_count_is_rejected(self):
        message = error(VALID.replace("bus_bits: 2", "bus_bits: 2.0", 1))
        self.assertIn("must be an integer", message)

    def test_decimal_page_size_is_rejected(self):
        altered = VALID.replace("page_size: 2048", "page_size: 2048.0", 1)
        message = error(altered)
        self.assertIn("must be an integer", message)

    def test_tuning_must_be_a_section(self):
        message = error(
            VALID.replace(
                "tuning:\n  gc_ratio: 0.1\n  gc_all: 1.0\n"
                "  gc_threshold: 0.2\n",
                "tuning: 1\n",
            )
        )
        self.assertIn("'tuning' must be a section", message)

    def test_tuning_value_must_be_a_number(self):
        message = error(
            VALID.replace("  gc_ratio: 0.1", "  gc_ratio:\n    nested: 1")
        )
        self.assertIn("must be a number", message)

    def test_geometry_must_be_a_section(self):
        message = error(VALID[: VALID.index("geometry:")] + "geometry: 1\n")
        self.assertIn("'geometry' must be a section", message)

    def test_backend_must_be_a_section(self):
        message = error(
            VALID.replace(
                "  ramdisk:\n    bus_bits: 2\n    chip_bits: 2\n"
                "    page_bits: 7\n    block_bits: 19\n    page_size: 8192\n",
                "  ramdisk: 1\n",
            )
        )
        self.assertIn("'geometry.ramdisk' must be a section", message)

    def test_main_rejects_an_unknown_argument(self):
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            self.assertEqual(gen_config.main(["--bogus"]), 2)
        self.assertIn("usage:", err.getvalue())


class GenerationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / "src").mkdir()
        (self.root / "include").mkdir()
        (self.root / "ftl.yaml").write_text(VALID, encoding="utf-8")

    def tearDown(self):
        self.temp.cleanup()

    def test_generates_both_files_and_is_idempotent(self):
        written = gen_config.generate(self.root)
        self.assertEqual(
            written, ["src/ftl_config.rs", "include/ftl-config.h"]
        )
        rust = (self.root / "src" / "ftl_config.rs").read_text()
        header = (self.root / "include" / "ftl-config.h").read_text()
        self.assertIn("pub const PAGE_FTL_GC_RATIO: f64 = 0.1;", rust)
        self.assertIn("pub const DEVICE_PAGE_SIZE: usize = 2048;", rust)
        self.assertIn("#define DEVICE_PAGE_SIZE (2048)", header)
        self.assertIn("#ifndef FTL_CONFIG_H", header)
        self.assertEqual(gen_config.generate(self.root), [])

    def test_values_come_from_the_yaml(self):
        (self.root / "ftl.yaml").write_text(
            VALID.replace("gc_ratio: 0.1", "gc_ratio: 0.25").replace(
                "page_size: 2048", "page_size: 4096"
            ),
            encoding="utf-8",
        )
        gen_config.generate(self.root)
        rust = (self.root / "src" / "ftl_config.rs").read_text()
        header = (self.root / "include" / "ftl-config.h").read_text()
        self.assertIn("pub const PAGE_FTL_GC_RATIO: f64 = 0.25;", rust)
        self.assertIn("#define DEVICE_PAGE_SIZE (4096)", header)

    def test_check_detects_a_stale_generated_file(self):
        gen_config.generate(self.root)
        self.assertEqual(gen_config.check(self.root), 0)
        path = self.root / "include" / "ftl-config.h"
        path.write_text(path.read_text() + "/* drift */\n", encoding="utf-8")
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(
            io.StringIO()
        ):
            self.assertEqual(gen_config.check(self.root), 1)
        self.assertIn("-/* drift */", out.getvalue())
        expected = ["include/ftl-config.h"]
        self.assertEqual(gen_config.generate(self.root), expected)
        self.assertEqual(gen_config.check(self.root), 0)

    def test_check_detects_a_changed_yaml(self):
        gen_config.generate(self.root)
        (self.root / "ftl.yaml").write_text(
            VALID.replace("gc_all: 1.0", "gc_all: 0.5"), encoding="utf-8"
        )
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(
            io.StringIO()
        ):
            self.assertEqual(gen_config.check(self.root), 1)
        self.assertIn("PAGE_FTL_GC_ALL", out.getvalue())


if __name__ == "__main__":
    unittest.main()
