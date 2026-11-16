import java.util.AbstractList;
import java.util.Comparator;

class Main {
    abstract static class Shape {
        abstract int area();
        public String label() { return "shape"; }
    }

    void anonymous() {
        Runnable bad = new /*@anon_missing*/Runnable() {
            public void other() {}
        };
        Runnable ok1 = new Runnable() {
            public void run() {}
        };
        Comparator<String> ok2 = new Comparator<String>() {
            public int compare(String a, String b) { return 0; }
        };
        Shape bad2 = new /*@anon_shape*/Shape() {
            int perimeter() { return 0; }
        };
        Shape ok3 = new Shape() {
            int area() { return 1; }
        };
        Object ok4 = new AbstractList<String>() {
            public String get(int i) { return ""; }
            public int size() { return 0; }
        };
        Comparator<String> ok5 = new Comparator<>() {
            public int compare(String a, String b) { return 0; }
        };
    }

    void local() {
        class /*@local_missing*/Square extends Shape {
            int side;
        }
        class Circle extends Shape {
            int area() { return 3; }
            String /*@weaker*/label() { return "circle"; }
        }
        abstract class Partial extends Shape {}
        class Full extends Partial {
            int area() { return 2; }
            public String label() { return "full"; }
        }
    }
}
//? diag missing-implementation anon_missing
//? diag missing-implementation anon_shape
//? diag missing-implementation local_missing
//? diag invalid-override weaker
//? diag exact
