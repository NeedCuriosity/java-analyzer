import static java.lang.Math.max;
import static java.util.Collections.emptyList;

import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

public class Statics {
    private static final int LIMIT = 10;
    private static final AtomicInteger COUNTER = new AtomicInteger();
    private static int created;
    static final String PREFIX;

    static {
        PREFIX = "id-";
    }

    private final int id;
    private final String label;

    {
        created++;
    }

    public Statics() {
        this(COUNTER.incrementAndGet());
    }

    public Statics(int id) {
        this.id = id;
        this.label = PREFIX + id;
    }

    public static int clamp(int value) {
        int bounded = max(value, 0);
        return Math.min(bounded, LIMIT);
    }

    public static List<String> none() {
        List<String> empty = emptyList();
        return empty;
    }

    public String label() {
        String text = label + ":" + created;
        return text;
    }

    public long big() {
        long millis = System.currentTimeMillis();
        int hash = Integer.hashCode(id);
        String hex = Integer.toHexString(hash);
        char first = hex.charAt(0);
        boolean digit = Character.isDigit(first);
        return digit ? millis : Long.MAX_VALUE;
    }

    public static void main(String[] args) {
        Statics one = new Statics();
        int clamped = clamp(args.length);
        System.out.println(one.label() + clamped);
    }
}
