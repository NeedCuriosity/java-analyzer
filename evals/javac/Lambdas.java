import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.function.BiFunction;
import java.util.function.Function;
import java.util.function.Predicate;
import java.util.function.Supplier;
import java.util.stream.Collectors;

public class Lambdas {
    interface Visitor<R> {
        R visit(String node, int depth);
    }

    private final List<String> log = new ArrayList<>();

    public List<Integer> lengths(List<String> names) {
        Function<String, Integer> length = String::length;
        Predicate<String> nonEmpty = s -> !s.isEmpty();
        List<Integer> result = names.stream().filter(nonEmpty).map(length).collect(Collectors.toList());
        return result;
    }

    public Map<Integer, List<String>> byLength(List<String> names) {
        Map<Integer, List<String>> grouped = names.stream().collect(Collectors.groupingBy(String::length));
        return grouped;
    }

    public String first(List<String> names) {
        Optional<String> found = names.stream().filter(n -> n.startsWith("a")).findFirst();
        String value = found.map(String::toUpperCase).orElse("none");
        return value;
    }

    public int apply(int base) {
        BiFunction<Integer, Integer, Integer> add = (a, b) -> a + b;
        Supplier<Integer> seed = () -> base * 2;
        Visitor<String> visitor = (node, depth) -> node + depth;
        Runnable record = () -> log.add(visitor.visit("n", base));
        record.run();
        int sum = add.apply(seed.get(), base);
        return sum;
    }

    public long count(List<String> names, int limit) {
        long total = names.stream()
                .mapToInt(String::length)
                .filter(n -> n > limit)
                .count();
        return total;
    }

    public String joined(List<String> names) {
        StringBuilder builder = new StringBuilder();
        names.forEach(name -> {
            String trimmed = name.trim();
            builder.append(trimmed);
        });
        return builder.toString();
    }
}
