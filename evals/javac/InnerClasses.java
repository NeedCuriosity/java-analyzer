import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;

public class InnerClasses implements Iterable<String> {
    private final List<String> items = new ArrayList<>();
    private int modifications;

    class Cursor implements Iterator<String> {
        private int position;

        @Override
        public boolean hasNext() {
            return position < items.size();
        }

        @Override
        public String next() {
            String current = items.get(position);
            position++;
            return current;
        }
    }

    static class Counter {
        private int value;

        int increment() {
            value++;
            return value;
        }
    }

    @Override
    public Iterator<String> iterator() {
        return new Cursor();
    }

    public void add(String item) {
        items.add(item);
        modifications++;
    }

    public Runnable watcher() {
        Counter counter = new Counter();
        return new Runnable() {
            private int seen;

            @Override
            public void run() {
                seen = counter.increment() + modifications;
                System.out.println(seen);
            }
        };
    }

    public int localClass(int start) {
        class Accumulator {
            int sum = start;

            void accept(String text) {
                sum += text.length();
            }
        }
        Accumulator acc = new Accumulator();
        for (String item : this) {
            acc.accept(item);
        }
        int result = acc.sum;
        return result;
    }

    public int outerAccess() {
        Cursor cursor = this.new Cursor();
        int count = 0;
        while (cursor.hasNext()) {
            String value = cursor.next();
            count += value.length();
        }
        return count + InnerClasses.this.modifications;
    }
}
