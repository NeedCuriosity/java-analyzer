import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.Optional;

class Main {
    static <T> T first(List<T> xs) { return xs.get(0); }
    static <A, B> B second(A a, B b) { return b; }
    static <T extends Comparable<T>> T top(T a, T b) { return a; }

    void run(List<String> names, List<? extends Number> numbers) {
        var /*@a*/a = first(names);
        var /*@b*/b = second(1, "x");
        var /*@c*/c = top("p", "q");
        var /*@d*/d = Collections.max(names);
        var /*@e*/e = Arrays.asList(1, 2, 3);
        var /*@f*/f = List.of("a", "b");
        var /*@g*/g = Optional.of(names);
        var /*@h*/h = g.get().get(0);
        var /*@i*/i = numbers.get(0);
        var /*@j*/j = Collections.emptyList();
        var /*@k*/k = Optional.ofNullable(names.get(0)).orElse("none");
        List<String> /*@l*/l = Collections.emptyList();
    }
}
//? hover a ~ "String a"
//? hover b ~ "String b"
//? hover c ~ "String c"
//? hover d ~ "String d"
//? hover e ~ "List<Integer> e"
//? hover f ~ "List<String> f"
//? hover g ~ "Optional<List<String>> g"
//? hover h ~ "String h"
//? hover i ~ "Number i"
//? hover k ~ "String k"
//? diag none
