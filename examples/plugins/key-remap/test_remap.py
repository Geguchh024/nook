import remap, unittest

class T(unittest.TestCase):
    def test_parse(self):
        self.assertEqual(remap.parse("ctrl+alt+KeyQ"), (3, 0x51))
        self.assertEqual(remap.parse("Ctrl+Shift+q"), (6, 0x51))
        self.assertEqual(remap.parse("alt+Digit1"), (1, 0x31))
        self.assertEqual(remap.parse("win+F5"), (8, 0x74))
        self.assertEqual(remap.parse("ctrl+space"), (2, 0x20))
    def test_bad(self):
        for s in ["", "q", "ctrl+", "foo+q", "ctrl+banana", "ctrl+F99"]:
            with self.assertRaises(ValueError, msg=s):
                remap.parse(s)
    def test_pretty(self):
        self.assertEqual(remap.pretty("ctrl+shift+KeyQ"), "Ctrl+Shift+Q")
        self.assertEqual(remap.pretty("alt+f5"), "Alt+F5")
        self.assertEqual(remap.pretty("ctrl+escape"), "Ctrl+Escape")
    def test_plan(self):
        # user holds Ctrl+Shift, we need Ctrl+Alt+Q
        ev = remap.plan(2 | 4, "ctrl+alt+KeyQ")
        self.assertEqual(ev[:2], [(0x11, False), (0x10, False)])
        self.assertIn((0x51, True), ev)
        self.assertEqual(ev[-2:], [(0x12, False), (0x11, False)])
        downs = [v for v, d in ev if d]
        self.assertEqual(downs, [0x11, 0x12, 0x51])
    def test_check(self):
        es = [{"id": "a", "name": "A", "old": "ctrl+alt+KeyA", "new": "ctrl+shift+KeyX"},
              {"id": "b", "name": "B", "old": "ctrl+alt+KeyB", "new": ""}]
        self.assertIn("already used", remap.check(es, "b", "ctrl+shift+x"))
        self.assertIn("original", remap.check(es, "b", "ctrl+alt+b"))
        self.assertIsNone(remap.check(es, "b", "ctrl+shift+y"))
        self.assertIsNone(remap.check(es, "a", "ctrl+shift+x"))
    def test_defaults(self):
        ids = {i for i, _, _ in remap.DEFAULT_TOOLS}
        self.assertEqual(remap.parse(remap.DEFAULT_NEW["qr"]), (6, 0x51))
        self.assertTrue(set(remap.DEFAULT_NEW) <= ids)

unittest.main()
