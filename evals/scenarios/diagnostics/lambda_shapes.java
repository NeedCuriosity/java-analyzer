import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.Callable;
import java.util.concurrent.ExecutorService;
import java.util.function.BiFunction;
import java.util.function.Consumer;
import java.util.function.Function;
import java.util.function.Predicate;
import java.util.function.Supplier;
import java.util.function.ToIntFunction;
import java.util.function.UnaryOperator;

class Main {
    interface Named {
        String name(int index);

        default String first() {
            return name(0);
        }
    }

    interface Transformer<T> extends Function<T, T> {
    }

    private int count;
    private Runnable task = () -> /*@field*/"x";

    void f() {
    }

    void run(Runnable r) {
        r.run();
    }

    Runnable make() {
        return () -> /*@ret*/1;
    }

    void uses(List<String> names, Map<String, List<String>> groups, ExecutorService executor, boolean flag) {
        Runnable r1 = () -> /*@expr_value*/5;
        Runnable r2 = /*@arity*/x -> {};
        names.removeIf(s -> /*@predicate*/s.length());
        Runnable r3 = () -> /*@block_value*/{ return 5; };
        Supplier<Integer> s1 = () -> /*@missing*/{};
        Supplier<Integer> s2 = () -> /*@wrong_type*/"x";
        Supplier<Integer> s3 = () -> /*@void_call*/f();
        Function<String, Integer> f1 = s -> {
            return /*@block_type*/s.isEmpty();
        };
        run(/*@arg_arity*/(a, b) -> {});
        r1 = () -> /*@assign*/"s";
        Supplier<Runnable> nested = () -> () -> /*@nested*/5;
        Named named = /*@custom_arity*/() -> "n";

        Runnable ok1 = () -> System.out.println();
        Runnable ok2 = () -> count++;
        Runnable ok3 = () -> new Object();
        Runnable ok4 = () -> {
            return;
        };
        Supplier<Integer> ok5 = () -> 5;
        Function<String, Integer> ok6 = String::length;
        Function<String, Integer> ok7 = s -> s.length();
        Predicate<String> ok8 = s -> s.isEmpty();
        names.removeIf(s -> s.isEmpty());
        executor.submit(() -> 5);
        executor.submit(() -> System.out.println());
        Supplier<Byte> ok9 = () -> 1;
        Supplier<Number> ok10 = () -> 1;
        Supplier<Object> ok11 = () -> {
            throw new RuntimeException();
        };
        Supplier<Integer> ok12 = () -> {
            while (true) {
            }
        };
        Callable<Integer> ok13 = () -> {
            if (flag) {
                return 1;
            }
            return 2;
        };
        Consumer<String> ok14 = s -> names.add(s);
        BiFunction<Integer, Integer, Integer> ok15 = (a, b) -> a + b;
        Comparator<String> ok16 = (a, b) -> a.length() - b.length();
        names.forEach(s -> System.out.println(s));
        names.sort((a, b) -> a.compareTo(b));
        Optional<Integer> ok17 = Optional.of("x").map(s -> s.length());
        Thread ok18 = new Thread(() -> System.out.println());
        groups.computeIfAbsent("k", k -> new ArrayList<>()).add("v");
        Function<String, ? super Integer> ok19 = s -> 1;
        ToIntFunction<String> ok20 = s -> s.length();
        Callable<Void> ok21 = () -> null;
        UnaryOperator<String> ok22 = s -> s.trim();
        Transformer<String> ok23 = s -> s + "!";
        Named ok24 = i -> "n" + i;
        Supplier<int[]> ok25 = () -> new int[0];
        Supplier<Supplier<String>> ok26 = () -> () -> "s";
        run(() -> f());
        run(() -> {
            if (flag) {
                return;
            }
            f();
        });
        System.out.println(r2 + "" + r3 + s1 + s2 + s3 + f1 + nested + named + ok1 + ok2 + ok3 + ok4 + ok5
                + ok6 + ok7 + ok8 + ok9 + ok10 + ok11 + ok12 + ok13 + ok14 + ok15 + ok16 + ok17 + ok18 + ok19
                + ok20 + ok21 + ok22 + ok23 + ok24 + ok25 + ok26 + task);
    }
}
//? diag type-mismatch field
//? diag type-mismatch ret
//? diag type-mismatch expr_value
//? diag type-mismatch arity
//? diag type-mismatch predicate
//? diag type-mismatch block_value
//? diag type-mismatch missing
//? diag type-mismatch wrong_type
//? diag type-mismatch void_call
//? diag type-mismatch block_type
//? diag type-mismatch arg_arity
//? diag type-mismatch assign
//? diag type-mismatch nested
//? diag type-mismatch custom_arity
//? diag exact
//? diag only type-mismatch
