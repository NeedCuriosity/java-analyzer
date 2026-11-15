import java.io.IOException;
import java.util.List;

interface Store<T> {
    T load(String key) throws IOException;
    void save(String key, T value);
    int size();
    static Store<String> empty() { return null; }
}

class CachedStore {
    private final Store<List<String>> /*@field*/backend;

    CachedStore(Store<List<String>> backend) {
        this.backend = backend;
    }

    public int size() {
        return 0;
    }
}
//? assist generate_delegates field ; has "    public List<String> load(String key) throws IOException {\n        return backend.load(key);\n    }" ; has "    public void save(String key, List<String> value) {\n        backend.save(key, value);\n    }" ; lacks "backend.size()" ; lacks "backend.empty" ; lacks "hashCode"
