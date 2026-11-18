import java.util.ArrayList;
import java.util.List;

abstract class Ordering<T> {
    static <T> Ordering<T> allEqual() { return null; }
    abstract Ordering<T> reverse();
    abstract T min(T a, T b);
}

class Main {
    void run() {
        List.of()./*@afterOf*/;
        new ArrayList<>()./*@afterNew*/;
        Ordering.allEqual().reverse()./*@afterOrdering*/;
        List.of().get(0)./*@element*/;
        Ordering.allEqual().min(null, null)./*@min*/;
    }
}
//? complete afterOf has size get stream
//? complete afterNew has add ensureCapacity
//? complete afterOrdering has reverse min
//? complete element has hashCode equals
//? complete min has hashCode equals
