class Patterns {
    int bad1(Object o) { if (o instanceof String s) {} return /*@leak*/s.length(); }

    int bad2(Object o) { if (o instanceof String s) { return 1; } else { return /*@else_leak*/s.length(); } }

    int bad3(Object o) { boolean b = o instanceof String s; return /*@decl_leak*/s.length(); }

    int bad4(Object o) { if (o instanceof String s || o == null) { return /*@or_leak*/s.length(); } return 0; }

    int ok1(Object o) { if (o instanceof String s && s.length() > 0) { return s.length(); } return 0; }

    int ok2(Object o) { if (!(o instanceof String s)) { return 0; } return s.length(); }

    int ok3(Object o) {
        if (o instanceof String s) { return s.length(); }
        if (o instanceof Integer s) { return s; }
        return 0;
    }

    int ok4(Object o) { if (!(o instanceof String s)) throw new IllegalArgumentException(); return s.length(); }

    int ok5(Object o) { return o instanceof String s ? s.length() : 0; }

    int ok6(Object o) { if (!(o instanceof String s)) { return 0; } else { return s.length(); } }

    int ok7(Object o) { while (!(o instanceof String s)) { o = o.toString(); } return s.length(); }

    boolean ok8(Object o) { return !(o instanceof String s) || s.isEmpty(); }

    int ok9(Object o) { if (o instanceof String s) return s.length(); else return 0; }

    int ok10(Object o) {
        for (int i = 0; i < 2; i++) {
            if (!(o instanceof String s)) { continue; }
            return s.length();
        }
        return 0;
    }

    int ok11(Object o, Object p) {
        if (!(o instanceof String s) || !(p instanceof String t)) { return 0; }
        return s.length() + t.length();
    }

    int ok12(Object o) { int n = o instanceof String s && !s.isEmpty() ? s.length() : 0; return n; }
}
//? diag unresolved-name leak
//? diag unresolved-name else_leak
//? diag unresolved-name decl_leak
//? diag unresolved-name or_leak
//? diag exact
