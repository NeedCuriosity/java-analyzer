import java.util.List;

interface DemoService {
    int createBrand(String name);
    int updateBrand(long id, String name);
    int deleteBrand(long id);
    List<String> listAllBrand();
    List<String> listBrand(int pageNum, int pageSize);
    String getBrand(long id);
}

class Base {
    protected void inherited() {}
}

class DemoController extends Base {
    private DemoService demoService;
    private int pageSize;

    void own() {}

    void run() {
        demoService./*@svc*/;
        this./*@self*/;
        int affected = demoService./*@typed*/;
    }
}
//? complete svc has createBrand updateBrand deleteBrand listAllBrand listBrand getBrand hashCode
//? complete svc order createBrand hashCode
//? complete svc order updateBrand equals
//? complete svc order deleteBrand toString
//? complete svc order listAllBrand getClass
//? complete svc order listBrand wait
//? complete svc order getBrand notify
//? complete self has own inherited pageSize clone
//? complete self order own hashCode
//? complete self order pageSize hashCode
//? complete self order inherited hashCode
