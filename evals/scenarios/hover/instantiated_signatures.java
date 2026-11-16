import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;

class Main {
    int run(Map<String, Integer> counts, List<String> names) {
        Integer n = counts./*@get*/get("a");
        List<Integer> sizes = names.stream()./*@map*/map(String::length).collect(Collectors.toList());
        names./*@add*/add("x");
        String joined = String./*@join*/join(",", names);
        return n + sizes.size() + joined.length();
    }
}
//? hover get ~ "Integer get(Object key)"
//? hover get ~ "V get(Object key)"
//? hover map ~ "Stream<Integer> map(Function<? super String, ? extends Integer> mapper)"
//? hover add ~ "boolean add(String e)"
//? hover join ~ "static String join(CharSequence delimiter, Iterable<? extends CharSequence> elements)"
//? diag none
