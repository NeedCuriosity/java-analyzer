import java.util.List;
import java.util.Objects;

class Main {
    String field;

    int definite(String s) {
        String a = null;
        int n = /*@deref_null*/a.length();
        if (s == null) {
            return /*@deref_in_branch*/s.length();
        }
        return n + s.length();
    }

    int elseBranch(String s) {
        if (s != null) {
            return s.length();
        } else {
            return /*@deref_else*/s.length();
        }
    }

    int afterAssign(String s, int[] xs) {
        s = null;
        xs = null;
        return /*@deref_assigned*/s.length() + /*@deref_array*/xs[0];
    }

    void constant(String s, Object o) {
        String fresh = new String("x");
        if (/*@always_true*/fresh != null) {}
        String none = null;
        if (/*@always_true2*/none == null) {}
        if (s == null) {
            return;
        }
        if (/*@always_false*/s == null) {}
        int n = o.hashCode();
        if (/*@after_deref*/o == null) {}
        String t = Objects.requireNonNull(field);
        Objects.requireNonNull(t);
        if (/*@after_require*/t != null) {}
        if (o instanceof String str && /*@in_and*/s != null) {}
        boolean b = /*@in_or*/s != null || s == null;
    }

    int fine(String s, String t, List<String> names, boolean flag) {
        if (s == null || s.isEmpty()) {
            return 0;
        }
        if (t != null && t.length() > 1) {
            return 1;
        }
        String found = null;
        for (String name : names) {
            if (name.isEmpty()) {
                found = name;
            }
        }
        if (found != null) {
            return found.length();
        }
        String maybe = null;
        if (flag) {
            maybe = "x";
        }
        if (maybe == null) {
            return 2;
        }
        String viaTry = null;
        try {
            viaTry = names.get(0);
        } catch (RuntimeException e) {
            if (viaTry == null) return 3;
        } finally {
            if (viaTry != null) viaTry.trim();
        }
        String loop = null;
        while (flag) {
            if (loop != null) {
                return loop.length();
            }
            loop = t;
        }
        String sw = null;
        switch (names.size()) {
            case 1: sw = "a"; break;
            default:
        }
        if (sw != null) return 4;
        if (field == null) {
            field = "x";
        }
        if (field == null) return 5;
        assert t != null;
        if (t == null) return 6;
        String lazy = null;
        String cap = names.get(0);
        Runnable r = () -> { if (cap != null) cap.length(); };
        r.run();
        lazy = flag ? "a" : null;
        return lazy == null ? 7 : lazy.length();
    }
}
//? diag null-dereference deref_null
//? diag null-dereference deref_in_branch
//? diag null-dereference deref_else
//? diag null-dereference deref_assigned
//? diag null-dereference deref_array
//? diag constant-condition always_true
//? diag constant-condition always_true2
//? diag constant-condition always_false
//? diag constant-condition after_deref
//? diag constant-condition after_require
//? diag constant-condition in_and
//? diag constant-condition in_or
//? diag only null-dereference
//? diag only constant-condition
//? diag none
