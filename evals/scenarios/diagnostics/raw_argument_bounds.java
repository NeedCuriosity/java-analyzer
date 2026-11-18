import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

class Main {
    static class Chunk implements Comparable {
        public int compareTo(Object o) {
            return 0;
        }
    }

    static <T extends Number> T first(List<T> xs) {
        return xs.get(0);
    }

    ArrayList chunks() {
        return new ArrayList();
    }

    void raw() {
        ArrayList list = chunks();
        Collections./*@sort*/sort(/*@sort_arg*/list);
        Number n = /*@first*/first(new ArrayList());
        System.out.println(n);
    }

    void typed(ArrayList<Chunk> chunks, List<Object> objects) {
        Collections.sort(chunks);
        Collections./*@bad_sort*/sort(objects);
    }
}
//? diag unchecked sort
//? diag unchecked sort_arg
//? diag unchecked first
//? diag argument-mismatch bad_sort
//? diag exact
