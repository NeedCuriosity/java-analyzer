import java.util.HashMap;
import java.util.function.IntSupplier;

class Main {
    int counter() {
        var /*@counter*/counter = new Object() {
            int /*@ndecl*/n;
            void /*@incdecl*/inc() { /*@n_in_inc*/n++; }
            int /*@getdecl*/get() { return /*@n_in_get*/n; }
        };
        counter./*@inc1*/inc();
        counter./*@inc2*/inc();
        int /*@a*/a = counter./*@n_out*/n;
        IntSupplier s = () -> counter./*@get_lambda*/get();
        String text = counter.toString();
        return a + s.getAsInt() + text.length();
    }

    int direct() {
        return new Object() {
            int /*@twicedecl*/twice(int x) { return x * 2; }
        }./*@twice*/twice(21);
    }

    int generic() {
        var /*@map*/map = new HashMap<String, Integer>() {
            int /*@totaldecl*/total() { return values().stream().mapToInt(Integer::intValue).sum(); }
        };
        map.put("a", 1);
        Integer /*@one*/one = map.get("a");
        return map./*@total*/total() + one;
    }

    void members() {
        var c = new Runnable() {
            int count;
            public void run() { count++; }
        };
        c./*@comp*/run();
    }

    void missing() {
        var c = new Object() { int n; };
        c./*@missing_m*/zzz();
        int v = c./*@missing_f*/m;
    }
}
//? hover counter ~ "<anonymous Object> counter"
//? hover a ~ "int a"
//? hover map ~ "<anonymous HashMap<String, Integer>> map"
//? hover one ~ "Integer one"
//? def inc1 -> incdecl
//? def n_out -> ndecl
//? def n_in_inc -> ndecl
//? def get_lambda -> getdecl
//? def twice -> twicedecl
//? def total -> totaldecl
//? hover inc1 ~ "void inc()"
//? hover n_out ~ "int n"
//? complete comp has run count toString hashCode
//? refs incdecl = inc1 inc2
//? refs ndecl = n_in_inc n_in_get n_out
//? diag unresolved-method missing_m
//? diag unresolved-field missing_f
//? diag exact
