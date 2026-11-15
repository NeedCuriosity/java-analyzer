import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

class Main {
    void writeOnly(List<String> input) {
        List<String> /*@write*/seen = new ArrayList<>();
        for (String s : input) {
            seen.add(s);
        }
    }

    boolean readOnly(String key) {
        Map<String, Integer> /*@read*/cache = new HashMap<>();
        return cache.containsKey(key);
    }

    List<String> returned(List<String> input) {
        List<String> out = new ArrayList<>();
        out.addAll(input);
        return out;
    }

    int used(List<String> input) {
        List<String> copy = new ArrayList<>(input.size());
        copy.addAll(input);
        return copy.size();
    }

    void passed(List<String> input) {
        List<String> sink = new ArrayList<>();
        fill(sink);
        System.out.println(sink.size());
    }

    void fill(List<String> target) {
        target.add("x");
    }

    void fromElsewhere(List<String> input) {
        List<String> view = input;
        System.out.println(view.isEmpty());
    }

    boolean removeUsed(List<String> input) {
        List<String> tmp = new ArrayList<>(input);
        return tmp.remove("a");
    }

    void visited(List<String> input) {
        java.util.Set<String> seen = new java.util.HashSet<>();
        for (String s : input) {
            if (seen.add(s)) {
                System.out.println(s);
            }
        }
    }

    void shared(Map<String, List<String>> index) {
        List<String> bucket = index.get("a");
        bucket.add("b");
    }

    void lambda(List<String> input) {
        List<String> found = new ArrayList<>();
        input.forEach(s -> found.add(s));
        found.forEach(System.out::println);
    }
}
//? diag mismatched-collection write
//? diag mismatched-collection read
//? diag only mismatched-collection
