import java.util.AbstractList;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.function.Function;
import java.util.function.Supplier;

interface Visitor {
    void visit(Integer value);
    void visit(String value);
}

interface Handler<T> {
    void handle(T item);
    void handle(T item, int times);
}

interface Converter {
    <T> T convert(Object value, Class<T> type);
}

class /*@visitor*/HalfVisitor implements Visitor {
    public void visit(Integer value) {}
}

class FullVisitor implements Visitor {
    public void visit(Integer value) {}
    public void visit(String value) {}
}

class /*@handler*/StringHandler implements Handler<String> {
    public void handle(String item) {}
    public void handle(Integer item, int times) {}
}

class GoodHandler implements Handler<String> {
    public void handle(String item) {}
    public void handle(String item, int times) {}
}

class GenericHandler<E> implements Handler<E> {
    public void handle(E item) {}
    public void handle(E item, int times) {}
}

class ByLength implements Comparator<String> {
    public int compare(String a, String b) { return a.length() - b.length(); }
}

class Parser implements Function<String, Integer> {
    public Integer apply(String text) { return text.length(); }
}

class GenericConverter implements Converter {
    public <U> U convert(Object value, Class<U> type) { return type.cast(value); }
}

class Names extends AbstractList<String> {
    public String get(int index) { return ""; }
    public int size() { return 0; }
}

class /*@numbers*/Numbers extends AbstractList<String> {
    public Integer /*@bad_get*/get(int index) { return 0; }
    public int size() { return 0; }
}

class Source implements Supplier<String> {
    public String get() { return ""; }
}

class /*@bad_source*/BadSource implements Supplier<String> {
    public Integer /*@bad_supplier*/get() { return 0; }
}

class ListSource implements Supplier<List<String>> {
    public ArrayList<String> get() { return new ArrayList<>(); }
}

class /*@wrong_list*/WrongListSource implements Supplier<List<String>> {
    public List<Integer> /*@bad_list*/get() { return new ArrayList<>(); }
}

enum /*@enum_missing*/Job implements Runnable {
    FIRST, SECOND;
}

enum GoodJob implements Runnable {
    FIRST;

    public void run() {}
}

enum Shape {
    SQUARE {
        double area() { return 1; }
    },
    /*@circle*/CIRCLE {
        double perimeter() { return 2; }
    };

    abstract double area();
}

enum Operation implements Runnable {
    PLUS {
        public void run() {}
        String /*@narrow*/label() { return "+"; }
    },
    MINUS {
        public void run() {}
    };

    public String label() { return name(); }
}

class PackageRunner {
    void run() {}
}

class /*@weak_impl*/WeakTask extends PackageRunner implements Runnable {}

class PublicRunner {
    public void run() {}
}

class GoodTask extends PublicRunner implements Runnable {}
//? diag missing-implementation visitor
//? diag missing-implementation handler
//? diag invalid-override bad_get
//? diag invalid-override bad_supplier
//? diag invalid-override bad_list
//? diag missing-implementation enum_missing
//? diag missing-implementation circle
//? diag invalid-override narrow
//? diag missing-implementation numbers
//? diag missing-implementation bad_source
//? diag missing-implementation wrong_list
//? diag invalid-override weak_impl
//? diag exact
