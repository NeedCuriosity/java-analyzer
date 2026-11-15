import java.io.Serializable;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

class Registry {
    private final List<String> /*@log*/log = new ArrayList<>();
    private final Map<String, Integer> /*@empty*/cache = new HashMap<>();
    private final List<String> names = new ArrayList<>();
    private final List<String> exposed = new ArrayList<>();
    public final List<String> visible = new ArrayList<>();

    void record(String event) {
        log.add(event);
        this.names.add(event);
    }

    Integer lookup(String key) {
        return cache.get(key);
    }

    int count() {
        return names.size();
    }

    List<String> exposed() {
        return exposed;
    }

    void addVisible(String s) {
        visible.add(s);
    }
}

class Saved implements Serializable {
    private final List<String> items = new ArrayList<>();

    void add(String s) {
        items.add(s);
    }
}
//? diag mismatched-collection log
//? diag mismatched-collection empty
//? diag only mismatched-collection
