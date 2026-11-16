import static java.util.Objects.requireNonNull;

class Main {
    void put(String key) {}
    void put(String key, Object value) {}
    void pick(String key, Object value, int index) {}
    void peek() {}

    void run(Object target) {
        Object checked = /*@static_import*/(target);
        this.p/*@arity*/("k", target, 1);
    }
}
//? complete static_import first requireNonNull
//? complete arity first pick
