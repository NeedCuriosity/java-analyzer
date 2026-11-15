import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

class Main {
    private final Map<String, List<Integer>> groups = new HashMap<>();

    int lookup(Map<String, Integer> counts, String name) {
        return /*@g1*/counts.containsKey(name) ? counts.get(name) : 0;
    }

    void defaults(Map<String, Integer> counts, String name, int initial) {
        if (!/*@p1*/counts.containsKey(name)) {
            counts.put(name, initial);
        }
        if (!counts.containsKey(name)) {
            counts.put(name, compute());
        }
        if (!counts.containsKey(name)) {
            counts.put(name, null);
        }
    }

    void add(String key, int value) {
        List<Integer> list = groups.get(key);
        if (/*@c1*/list == null) {
            list = new ArrayList<>();
            groups.put(key, list);
        }
        list.add(value);
    }

    void kept(Map<String, Integer> counts, String name, String other) {
        int a = counts.containsKey(name) ? counts.get(other) : 1;
        List<Integer> list = groups.get(name);
        if (list == null) {
            list = new ArrayList<>();
            System.out.println("created");
            groups.put(name, list);
        }
        System.out.println(a + list.size());
    }

    int compute() {
        return 1;
    }
}
//? diag map-api g1
//? diag map-api p1
//? diag map-api c1
//? diag only map-api
//? assist fix_redundancy g1 ; has "return counts.getOrDefault(name, 0);"
//? assist fix_redundancy p1 ; has "        counts.putIfAbsent(name, initial);\n        if (!counts.containsKey(name)) {"
//? assist fix_redundancy c1 ; has "List<Integer> list = groups.computeIfAbsent(key, k -> new ArrayList<>());\n        list.add(value);"
