import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;
import java.util.function.Function;
import java.util.stream.Collectors;
import java.util.stream.Stream;

class Main {
    static <T, R> R apply(T value, Function<T, R> f) {
        return f.apply(value);
    }

    void f() {
    }

    void uses(List<String> names, Optional<String> maybe, Map<String, Integer> counts) {
        names.stream().map(/*@two*/(a, b) -> a);
        names.stream().map(s -> /*@noValue*/{ System.out.println(s); });
        maybe.map(/*@none*/() -> 1);
        CompletableFuture.supplyAsync(/*@supplier*/x -> 1);
        Collections.sort(names, /*@comparator*/a -> 0);
        apply("x", s -> /*@block*/{ f(); });

        names.stream().map(s -> s.length()).forEach(n -> System.out.println(n));
        names.stream().flatMap(s -> Stream.of(s, s)).count();
        maybe.map(s -> s.isEmpty()).orElseGet(() -> false);
        CompletableFuture.runAsync(() -> f());
        CompletableFuture.supplyAsync(() -> 1).thenAccept(n -> System.out.println(n));
        names.stream().collect(Collectors.toMap(s -> s, s -> 1));
        names.stream().reduce("", (a, b) -> a + b);
        names.stream().map(s -> {
            throw new IllegalStateException(s);
        });
        apply(1, n -> n + 1);
        apply("x", s -> {
            return s.length();
        });
        counts.forEach((k, v) -> System.out.println(k + v));
        Collections.sort(names, (a, b) -> a.compareTo(b));
    }
}
//? diag type-mismatch two
//? diag type-mismatch noValue
//? diag type-mismatch none
//? diag type-mismatch supplier
//? diag type-mismatch comparator
//? diag type-mismatch block
//? diag exact
