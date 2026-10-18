using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Linq;
using System.Net.Http;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading;
using System.Threading.Tasks;
using System.Windows.Forms;
using System.Drawing.Drawing2D;
using System.Reflection;

namespace BuddyHub;
public static class Program
{
    public static readonly string Data=Environment.GetEnvironmentVariable("BUDDYHUB_DATA")??Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),".buddyhub");
    public static readonly string Root=AppContext.BaseDirectory;
    // Management traffic is always loopback; it must not depend on Windows/VPN proxy discovery.
    public static readonly HttpClient Http=new(new SocketsHttpHandler{UseProxy=false,ConnectTimeout=TimeSpan.FromSeconds(1),PooledConnectionLifetime=TimeSpan.FromMinutes(2),MaxConnectionsPerServer=4}){Timeout=Timeout.InfiniteTimeSpan};
    public static void Diagnostic(string message)
    {
        try{var file=Path.Combine(Data,"desktop-connection.log");if(File.Exists(file)&&new FileInfo(file).Length>100000)File.Move(file,file+".previous",true);File.AppendAllText(file,DateTime.Now.ToString("O")+" "+message+Environment.NewLine);}catch{}
    }
    public static JsonObject Config=>JsonNode.Parse(File.ReadAllText(Path.Combine(Data,"config.json")))!.AsObject();
    public static async Task<JsonNode> Api(string route,object? body=null)
    {
        var cfg=Config;using var req=new HttpRequestMessage(body==null?HttpMethod.Get:HttpMethod.Post,$"http://127.0.0.1:{cfg["adminPort"]}/admin/{route}");
        req.Headers.Authorization=new("Bearer",cfg["adminKey"]!.GetValue<string>());
        if(body!=null)req.Content=new StringContent(JsonSerializer.Serialize(body),Encoding.UTF8,"application/json");
        using var timeout=new CancellationTokenSource(TimeSpan.FromSeconds(route is "status" or "logs" or "client-config"?2:route=="sharing"?4:route is "checkin" or "balances"?180:75));
        HttpResponseMessage response;
        try{response=await Http.SendAsync(req,timeout.Token).ConfigureAwait(false);}
        catch(OperationCanceledException){Diagnostic(route+" timeout");throw new Exception("后台连接超时，将自动重试；也可以点击刷新");}
        using var r=response;var j=JsonNode.Parse(await r.Content.ReadAsStringAsync(timeout.Token).ConfigureAwait(false))!;
        if(!r.IsSuccessStatusCode)throw new Exception(j["error"]?.ToString()??"后台服务暂不可用");return j;
    }
    public static void Launch(string exe,params string[] args)
    {
        if(!File.Exists(exe))throw new Exception("找不到启动程序："+exe);
        var p=new ProcessStartInfo(exe){UseShellExecute=false,CreateNoWindow=true,WindowStyle=ProcessWindowStyle.Hidden,WorkingDirectory=Path.GetDirectoryName(exe)!};foreach(var a in args)p.ArgumentList.Add(a);Process.Start(p);
    }
    public static async Task EnsureBackend()
    {
        Diagnostic("检查本机后台");
        try{await Api("status");return;}catch{}
        Launch(Path.Combine(Root,"buddyhub-service.exe"),"--daemon");
        var elapsed=Stopwatch.StartNew();while(elapsed.Elapsed<TimeSpan.FromSeconds(10)){await Task.Delay(250);try{await Api("status");return;}catch{}}throw new Exception("暂时无法连接后台，窗口会自动重试，请稍后刷新");
    }
    static bool CheckinDue(JsonObject config)
    {
        if(config["checkinEnabled"]?.GetValue<bool>()!=true)return false;
        var now=TimeZoneInfo.ConvertTimeBySystemTimeZoneId(DateTimeOffset.UtcNow,"China Standard Time").ToString("HH:mm");
        return string.CompareOrdinal(now,config["checkinTime"]?.ToString()??"09:00")>=0;
    }
    [STAThread] public static void Main(string[] args)
    {
        if(args.Contains("--version")){Console.WriteLine("BuddyHub 1.0.1");return;}
        Directory.CreateDirectory(Data);
        Diagnostic("窗口版本 1.0.1 · 数据目录 "+Data+" · 程序 "+Environment.ProcessPath);
        if(args.Any(a=>a is "--background" or "--guard" or "--checkin"))
        {
            try{Launch(Path.Combine(Root,"buddyhub-service.exe"),args.Contains("--guard")?"--guard":args.Contains("--checkin")?"--checkin":"--ensure");}
            catch(Exception e){File.AppendAllText(Path.Combine(Data,"desktop-errors.log"),DateTime.Now+" "+e.Message+Environment.NewLine);}return;
        }
        Application.SetHighDpiMode(HighDpiMode.PerMonitorV2);Application.EnableVisualStyles();Application.SetCompatibleTextRenderingDefault(false);
        using var mutex=new Mutex(true,@"Local\BuddyHub.Desktop",out var first);if(!first){MessageBox.Show("BuddyHub 窗口已打开，请从任务栏切换到现有窗口。");return;}
        Application.Run(new HubForm());
    }
}

public sealed partial class HubForm:Form
{
    static readonly Color Bg=Color.FromArgb(245,247,251),PanelBg=Color.White,TextColor=Color.FromArgb(33,45,62),Muted=Color.FromArgb(117,131,149),Accent=Color.FromArgb(18,146,119),SidebarBg=Color.FromArgb(18,35,55);
    readonly Label footer=new(){Dock=DockStyle.Fill,Text="正在连接后台…",ForeColor=Muted,TextAlign=ContentAlignment.MiddleLeft};
    readonly Label overview=new(){Dock=DockStyle.Fill,ForeColor=TextColor,Padding=new Padding(20),Font=new Font("Microsoft YaHei UI",11)};
    readonly Label[] values=Enumerable.Range(0,4).Select(_=>new Label{Dock=DockStyle.Fill,Font=new Font("Microsoft YaHei UI",26,FontStyle.Bold),ForeColor=Accent,Text="—",TextAlign=ContentAlignment.MiddleLeft}).ToArray();
    readonly ListView cn=List(),ai=List(),services=List();
    readonly TextBox logs=new(){Dock=DockStyle.Fill,Multiline=true,ReadOnly=true,ScrollBars=ScrollBars.Both,WordWrap=false,BackColor=PanelBg,ForeColor=Muted,BorderStyle=BorderStyle.None,Font=new Font("Consolas",9)};
    readonly CheckBox autoSign=new(){Text="每日自动签到中国版",AutoSize=true,ForeColor=TextColor};
    readonly CheckBox startupGuard=new(){Text="保持 WorkBuddy 自启动关闭",AutoSize=true,ForeColor=TextColor};
    readonly TextBox signTime=new(){Text="09:00",Width=120};
    readonly ComboBox defaultPool=new(){DropDownStyle=ComboBoxStyle.DropDownList,Width=210};
    readonly Label pageTitle=new(){Text="概览",Font=new Font("Microsoft YaHei UI",22,FontStyle.Bold),AutoSize=true};
    readonly Label pageSubtitle=new(){Text="让每天的重复操作，安静地自动完成。",AutoSize=true,ForeColor=Muted};
    readonly Label runtimeBadge=new(){Text="后台连接中",AutoSize=true,ForeColor=Accent,Padding=new Padding(12,8,12,8),BackColor=Color.FromArgb(227,244,238)};
    readonly Panel pageHost=new(){Dock=DockStyle.Fill,BackColor=Bg};
    readonly List<Button> navigation=new();
    readonly List<Control> pages=new();
    readonly System.Windows.Forms.Timer timer=new(){Interval=12000};
    bool busy=false,refreshing=false,ready=false,dirty=false;
    public HubForm()
    {
        Text="BuddyHub · 本地 AI 管理";ClientSize=new Size(1220,760);MinimumSize=new Size(1120,720);StartPosition=FormStartPosition.CenterScreen;BackColor=Bg;ForeColor=TextColor;Font=new Font("Microsoft YaHei UI",10);DoubleBuffered=true;
        using(var stream=Assembly.GetExecutingAssembly().GetManifestResourceStream("BuddyHub.assets.buddyhub.ico")!)Icon=new Icon(stream);
        var shell=new TableLayoutPanel{Dock=DockStyle.Fill,ColumnCount=2,RowCount=1,Margin=Padding.Empty,Padding=Padding.Empty};shell.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute,208));shell.ColumnStyles.Add(new ColumnStyle(SizeType.Percent,100));Controls.Add(shell);
        var sidebar=new Panel{Dock=DockStyle.Fill,BackColor=SidebarBg,Margin=Padding.Empty};
        using(var stream=Assembly.GetExecutingAssembly().GetManifestResourceStream("BuddyHub.assets.buddyhub.png")!){var image=Image.FromStream(stream);sidebar.Controls.Add(new PictureBox{Image=new Bitmap(image),SizeMode=PictureBoxSizeMode.Zoom,Location=new Point(23,31),Size=new Size(42,42)});}
        sidebar.Controls.Add(new Label{Text="BuddyHub",AutoSize=true,Location=new Point(74,34),Font=new Font(Font.FontFamily,16,FontStyle.Bold),ForeColor=Color.White});sidebar.Controls.Add(new Label{Text="本地 AI 工作空间",AutoSize=true,Location=new Point(24,85),Font=new Font(Font.FontFamily,9),ForeColor=Color.FromArgb(143,165,186)});
        var nav=new FlowLayoutPanel{Location=new Point(14,150),Size=new Size(180,360),FlowDirection=FlowDirection.TopDown,WrapContents=false,BackColor=SidebarBg};
        var names=new[]{"概览","中国版账号","国际版账号","本地服务","公网共享","设置与日志"};var icons=new[]{"\uE80F","\uE716","\uE774","\uE968","\uE753","\uE713"};
        pages.AddRange(new[]{Dashboard(),AccountPage(cn,"cn"),AccountPage(ai,"ai"),ServicePage(),SharingPage(),SettingsPage()});
        for(int i=0;i<names.Length;i++){int index=i;var button=new Button{Text=names[i],Size=new Size(180,48),FlatStyle=FlatStyle.Flat,TextAlign=ContentAlignment.MiddleLeft,Padding=new Padding(47,0,0,0),Margin=new Padding(0,0,0,8),BackColor=SidebarBg,ForeColor=Color.FromArgb(183,199,216),Cursor=Cursors.Hand};button.FlatAppearance.BorderSize=0;button.FlatAppearance.MouseOverBackColor=Color.FromArgb(32,52,73);button.Paint+=(_,e)=>{using var f=new Font("Segoe MDL2 Assets",14);TextRenderer.DrawText(e.Graphics,icons[index],f,new Point(18,15),button.ForeColor);};button.Click+=(_,_)=>Navigate(index);navigation.Add(button);nav.Controls.Add(button);}sidebar.Controls.Add(nav);
        var sidebarNote=new Label{Text="静默运行 · 本机处理\r\n窗口关闭后，后台继续工作",ForeColor=Color.FromArgb(143,165,186),Font=new Font(Font.FontFamily,9),AutoSize=false,Height=72,Dock=DockStyle.Bottom,Padding=new Padding(23,0,12,15)};sidebar.Controls.Add(sidebarNote);shell.Controls.Add(sidebar,0,0);
        var root=new TableLayoutPanel{Dock=DockStyle.Fill,Padding=new Padding(30,25,30,10),RowCount=3,ColumnCount=1,Margin=Padding.Empty};root.RowStyles.Add(new RowStyle(SizeType.Absolute,108));root.RowStyles.Add(new RowStyle(SizeType.Percent,100));root.RowStyles.Add(new RowStyle(SizeType.Absolute,36));shell.Controls.Add(root,1,0);
        var header=new Panel{Dock=DockStyle.Fill};pageTitle.Location=new Point(0,5);pageSubtitle.Location=new Point(2,55);header.Controls.Add(pageTitle);header.Controls.Add(pageSubtitle);
        var right=new FlowLayoutPanel{Dock=DockStyle.Right,Width=280,FlowDirection=FlowDirection.RightToLeft,Padding=new Padding(0,12,0,0)};right.Controls.Add(Btn("刷新",Refresh));right.Controls.Add(runtimeBadge);header.Controls.Add(right);root.Controls.Add(header,0,0);root.Controls.Add(pageHost,0,1);root.Controls.Add(footer,0,2);Navigate(0);
        FormClosed+=(_,_)=>timer.Stop();
        Shown+=async(_,_)=>{ready=true;timer.Start();await Run(async()=>{await Task.Run(Program.EnsureBackend).WaitAsync(TimeSpan.FromSeconds(12));await Refresh();});};
        // Status polling must continue even while a startup or account operation is pending.
        timer.Tick+=async(_,_)=>await Refresh();
    }
    void Navigate(int index){pageHost.Controls.Clear();var page=pages[index];page.Dock=DockStyle.Fill;pageHost.Controls.Add(page);pageTitle.Text=new[]{"概览","中国版账号","国际版账号","本地服务","公网共享","设置与日志"}[index];pageSubtitle.Text=new[]{"让每天的重复操作，安静地自动完成。","每日签到、可用积分和账号轮询，一目了然。","独立管理国际版账号，与中国版分开调用。","把常用的本地反代，集中在一个地方管理。","给朋友独立的连接方式，自己掌握调用范围。","调整自动化行为，查看最近的运行记录。"}[index];for(int i=0;i<navigation.Count;i++){navigation[i].BackColor=i==index?Color.FromArgb(34,60,79):SidebarBg;navigation[i].ForeColor=i==index?Color.FromArgb(92,227,195):Color.FromArgb(183,199,216);}if(ready&&index==4)_=Run(RefreshSharing);}
    static ListView List(){var l=new ListView{Dock=DockStyle.Fill,View=View.Details,FullRowSelect=true,MultiSelect=false,HideSelection=false,BackColor=PanelBg,ForeColor=TextColor,BorderStyle=BorderStyle.None,OwnerDraw=true};var height=new ImageList{ImageSize=new Size(1,44)};l.SmallImageList=height;l.DrawColumnHeader+=(_,e)=>{using var bg=new SolidBrush(Color.FromArgb(237,241,247));e.Graphics.FillRectangle(bg,e.Bounds);TextRenderer.DrawText(e.Graphics,e.Header!.Text,l.Font,new Rectangle(e.Bounds.X+14,e.Bounds.Y,e.Bounds.Width-16,e.Bounds.Height),Muted,TextFormatFlags.VerticalCenter|TextFormatFlags.Left);};l.DrawItem+=(_,e)=>{if(l.View!=View.Details)e.DrawDefault=true;};l.DrawSubItem+=(_,e)=>{bool selected=e.Item!.Selected;using var bg=new SolidBrush(selected?Color.FromArgb(224,243,238):e.ItemIndex%2==0?Color.White:Color.FromArgb(250,251,253));e.Graphics.FillRectangle(bg,e.Bounds);var text=e.SubItem!.Text;var color=text is "有效" or "运行中" or "今日已签到" or "签到成功"?Accent:text is "签到失败" or "需重新登录"?Color.FromArgb(193,80,72):TextColor;TextRenderer.DrawText(e.Graphics,text,l.Font,new Rectangle(e.Bounds.X+14,e.Bounds.Y,e.Bounds.Width-16,e.Bounds.Height),color,TextFormatFlags.VerticalCenter|TextFormatFlags.Left|TextFormatFlags.EndEllipsis);};return l;}
    static FlowLayoutPanel Flow()=>new(){Dock=DockStyle.Fill,AutoSize=false,WrapContents=true,BackColor=Bg};
    static void AddTab(TabControl tabs,string name,Control child){var tab=new TabPage(name){BackColor=Bg,ForeColor=TextColor,Padding=new Padding(14)};tab.Controls.Add(child);tabs.TabPages.Add(tab);}
    Button Btn(string text,Func<Task> action){bool primary=text is "立即签到中国版" or "全部签到" or "保存设置" or "启动";var b=new Button{Text=text,AutoSize=true,MinimumSize=new Size(72,40),Height=40,FlatStyle=FlatStyle.Flat,BackColor=primary?Accent:Color.White,ForeColor=primary?Color.White:TextColor,Padding=new Padding(13,7,13,7),Margin=new Padding(0,0,8,8),Cursor=Cursors.Hand};b.FlatAppearance.BorderSize=primary?0:1;b.FlatAppearance.BorderColor=Color.FromArgb(221,227,236);b.FlatAppearance.MouseOverBackColor=primary?Color.FromArgb(14,129,105):Color.FromArgb(235,241,248);b.Click+=async(_,_)=>await Run(action);return b;}
    async Task Run(Func<Task> fn){if(busy)return;busy=true;footer.Text="正在处理…";try{await fn();if(!footer.Text.StartsWith("连接异常"))footer.Text="操作完成 · "+DateTime.Now.ToString("HH:mm:ss");}catch(Exception e){footer.Text=e.Message;Program.Diagnostic("操作失败 "+e.GetType().Name+" "+e.Message);}finally{busy=false;}}
    Control Dashboard()
    {
        var p=new TableLayoutPanel{Dock=DockStyle.Fill,ColumnCount=1,RowCount=4,Padding=Padding.Empty};p.RowStyles.Add(new RowStyle(SizeType.Absolute,132));p.RowStyles.Add(new RowStyle(SizeType.Absolute,260));p.RowStyles.Add(new RowStyle(SizeType.Absolute,68));p.RowStyles.Add(new RowStyle(SizeType.Percent,100));
        var cards=new TableLayoutPanel{Dock=DockStyle.Fill,ColumnCount=4,Margin=Padding.Empty};for(int i=0;i<4;i++){cards.ColumnStyles.Add(new ColumnStyle(SizeType.Percent,25));var card=new Surface{Dock=DockStyle.Fill,BackColor=PanelBg,Padding=new Padding(20,15,20,12),Margin=new Padding(0,0,i==3?0:14,16)};var label=new Label{Dock=DockStyle.Top,Height=30,Text=new[]{"中国版账号","国际版账号","今日已签到","运行中服务"}[i],ForeColor=Muted,Font=new Font(Font.FontFamily,9)};values[i].Font=new Font("Segoe UI",28,FontStyle.Bold);values[i].ForeColor=i==2?Accent:TextColor;card.Controls.Add(values[i]);card.Controls.Add(label);cards.Controls.Add(card,i,0);}p.Controls.Add(cards,0,0);
        var summary=new Surface{Dock=DockStyle.Fill,Padding=new Padding(23,18,23,15),Margin=new Padding(0,0,0,18),BackColor=PanelBg};summary.Controls.Add(overview);summary.Controls.Add(new Label{Text="自动化与连接",Dock=DockStyle.Top,Height=35,Font=new Font(Font.FontFamily,13,FontStyle.Bold),ForeColor=TextColor});overview.BackColor=PanelBg;overview.Padding=new Padding(0,12,0,0);overview.Font=new Font(Font.FontFamily,10);p.Controls.Add(summary,0,1);
        var buttons=Flow();buttons.Padding=new Padding(0,8,0,0);buttons.Controls.Add(Btn("立即签到中国版",async()=>{await Program.Api("checkin",new{});await Refresh();}));buttons.Controls.Add(Btn("导入当前登录",Import));buttons.Controls.Add(Btn("复制客户端连接配置",CopyConfig));p.Controls.Add(buttons,0,2);
        p.Controls.Add(new Label{Dock=DockStyle.Fill,ForeColor=Muted,Text="窗口只在你打开时运行，关闭即可释放界面内存。\r\n每日签到和反代在独立后台中继续工作，无需打开 WorkBuddy。",Padding=new Padding(0,6,0,0),Font=new Font(Font.FontFamily,9)},0,3);return p;
    }
    async Task Import(){await Program.Api("import",new{});await Refresh();}
    Control AccountPage(ListView list,string site)
    {
        foreach(var (name,width) in new[]{("账号",175),("尾号",80),("状态",95),("可用积分",100),("今日签到",115),("奖励",70),("调用",65),("成功",65),("失败",65)})list.Columns.Add(name,width);
        var p=new TableLayoutPanel{Dock=DockStyle.Fill,ColumnCount=1,RowCount=3};p.RowStyles.Add(new RowStyle(SizeType.Absolute,84));p.RowStyles.Add(new RowStyle(SizeType.Absolute,50));p.RowStyles.Add(new RowStyle(SizeType.Percent,100));
        var actions=Flow();actions.Controls.Add(Btn("打开 "+(site=="cn"?"中国版":"国际版")+" WorkBuddy",async()=>{var file=Program.Config[site=="cn"?"workbuddyExe":"workbuddyIntlExe"]?.GetValue<string>()??"";if(!File.Exists(file)){using var dialog=new OpenFileDialog{Title="选择已安装的 WorkBuddy 启动程序",Filter="启动程序|*.exe"};if(dialog.ShowDialog(this)!=DialogResult.OK)return;file=dialog.FileName;await Program.Api("workbuddy-path",new{site,path=file});}Process.Start(new ProcessStartInfo(file){UseShellExecute=true});}));actions.Controls.Add(Btn("导入当前登录",Import));
        if(site=="cn")actions.Controls.Add(Btn("全部签到",async()=>{await Program.Api("checkin",new{});await Refresh();}));
        actions.Controls.Add(Btn("查询余额",async()=>{var result=await Program.Api("balances",new{site});await Refresh();var failures=result["results"]!.AsArray().Count(j=>j?["error"]!=null);if(failures>0)throw new Exception($"{failures} 个账号余额查询失败，请稍后重试；签到记录仍有效");}));
        actions.Controls.Add(Btn("启用 / 停用",async()=>{var a=Selected(list);await Program.Api("account",new{id=a["id"]!.ToString(),enabled=!a["enabled"]!.GetValue<bool>()});await Refresh();}));
        actions.Controls.Add(Btn("重试账号",async()=>{var a=Selected(list);var id=a["id"]!.ToString();await Program.Api("account",new{id,reset=true});if(site=="cn")await Program.Api("checkin",new{id});await Refresh();}));
        actions.Controls.Add(Btn("改备注",async()=>{var a=Selected(list);var text=Prompt("账号备注",a["label"]!.ToString());if(text!=null){await Program.Api("account",new{id=a["id"]!.ToString(),label=text});await Refresh();}}));
        p.Controls.Add(actions,0,0);p.Controls.Add(new Label{Dock=DockStyle.Fill,ForeColor=Muted,Font=new Font(Font.FontFamily,9),Text=site=="cn"?"添加账号：先在 WorkBuddy 完成登录，再导入。已有账号会保留，每天自动签到。":"模型名加 -ai 调用国际版账号。此号池独立轮询，不参与中国版每日签到。",Padding=new Padding(0,4,0,0)},0,1);var card=new Surface{Dock=DockStyle.Fill,Padding=new Padding(1),BackColor=PanelBg,Margin=Padding.Empty};card.Controls.Add(list);p.Controls.Add(card,0,2);return p;
    }
    static JsonNode Selected(ListView view){if(view.SelectedItems.Count==0)throw new Exception("请先选择一个条目");return (JsonNode)view.SelectedItems[0].Tag!;}
    Control ServicePage()
    {
        foreach(var (name,width) in new[]{("服务",175),("状态",95),("本机地址",245),("启动方式",115),("说明",275)})services.Columns.Add(name,width);
        var p=new TableLayoutPanel{Dock=DockStyle.Fill,ColumnCount=1,RowCount=3};p.RowStyles.Add(new RowStyle(SizeType.Absolute,84));p.RowStyles.Add(new RowStyle(SizeType.Absolute,50));p.RowStyles.Add(new RowStyle(SizeType.Percent,100));var actions=Flow();
        foreach(var (label,action) in new[]{("启动","start"),("停止","stop"),("重启","restart")})actions.Controls.Add(Btn(label,async()=>{var s=Selected(services);await Program.Api("service",new{id=s["id"]!.ToString(),action});await Task.Delay(500);await Refresh();}));
        actions.Controls.Add(Btn("切换自动启动",async()=>{var s=Selected(services);await Program.Api("service-autostart",new{id=s["id"]!.ToString(),autoStart=!(s["autoStart"]?.GetValue<bool>()??false)});await Refresh();}));
        actions.Controls.Add(Btn("打开地址",()=>{var s=Selected(services);Process.Start(new ProcessStartInfo(s["url"]!.ToString()){UseShellExecute=true});return Task.CompletedTask;}));actions.Controls.Add(Btn("添加其他反代",AddService));
        p.Controls.Add(actions,0,0);p.Controls.Add(new Label{Dock=DockStyle.Fill,ForeColor=Muted,Font=new Font(Font.FontFamily,9),Text="选择一个服务即可启动、停止或重启。Docker 项目需要 Docker Desktop。"},0,1);var card=new Surface{Dock=DockStyle.Fill,Padding=new Padding(1),BackColor=PanelBg,Margin=Padding.Empty};card.Controls.Add(services);p.Controls.Add(card,0,2);return p;
    }
    Control SettingsPage()
    {
        var p=new TableLayoutPanel{Dock=DockStyle.Fill,ColumnCount=2};p.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute,355));p.ColumnStyles.Add(new ColumnStyle(SizeType.Percent,100));var options=new FlowLayoutPanel{Dock=DockStyle.Fill,FlowDirection=FlowDirection.TopDown,WrapContents=false};
        options.Controls.Add(autoSign);options.Controls.Add(new Label{Text="开机或跨日后自动补签，后台每分钟检查。",Width=325,Height=56,ForeColor=Muted,Margin=new Padding(0,16,0,8)});startupGuard.Margin=new Padding(0,18,0,14);options.Controls.Add(startupGuard);options.Controls.Add(new Label{Text="未带区域后缀时使用的号池",AutoSize=true,ForeColor=Muted});defaultPool.Items.AddRange(new object[]{"中国版","国际版"});options.Controls.Add(defaultPool);
        var actions=new FlowLayoutPanel{Width=335,Height=56,Margin=new Padding(0,16,0,0)};actions.Controls.Add(Btn("保存设置",async()=>{await Program.Api("settings",new{checkinEnabled=autoSign.Checked,startupGuard=startupGuard.Checked,checkinTime=signTime.Text.Trim(),defaultSite=defaultPool.SelectedIndex==1?"ai":"cn"});dirty=false;await Refresh();}));actions.Controls.Add(Btn("立即关闭自启",async()=>{await Program.Api("guard",new{});await Refresh();}));options.Controls.Add(actions);
        options.Controls.Add(new Label{Text="关机或离线时无法执行本地签到，会在当天恢复运行并联网后补签。",Width=325,Height=56,ForeColor=Muted,Margin=new Padding(0,10,0,8)});options.Controls.Add(Btn("打开数据目录",()=>{Process.Start(new ProcessStartInfo(Program.Data){UseShellExecute=true});return Task.CompletedTask;}));options.Controls.Add(new Label{Text="新增账号需在官方页面完成验证。\r\n余额与签到奖励以平台实际返回为准。\r\n具体可用模型取决于账号权限。",Width=325,Height=86,ForeColor=Muted,Margin=new Padding(0,20,0,8)});p.Controls.Add(options,0,0);p.Controls.Add(logs,1,0);
        autoSign.CheckedChanged+=(_,_)=>{if(ready)dirty=true;};startupGuard.CheckedChanged+=(_,_)=>{if(ready)dirty=true;};signTime.TextChanged+=(_,_)=>{if(ready)dirty=true;};defaultPool.SelectedIndexChanged+=(_,_)=>{if(ready)dirty=true;};return p;
    }
    async Task CopyConfig(){var c=await Program.Api("client-config");Clipboard.SetText($"base_url = {c["url"]}\r\napi_key = {c["key"]}\r\n中国版模型 = deepseek-v4.1-flash-cn\r\n国际版模型 = deepseek-v4.1-flash-ai");}
    string? Prompt(string title,string value)
    {
        using var dlg=new Form{Text=title,ClientSize=new Size(500,140),StartPosition=FormStartPosition.CenterParent,BackColor=Bg,ForeColor=TextColor,Font=Font,FormBorderStyle=FormBorderStyle.FixedDialog,MaximizeBox=false,MinimizeBox=false};var edit=new TextBox{Text=value,Location=new Point(20,22),Width=460};var ok=new Button{Text="确定",DialogResult=DialogResult.OK,Location=new Point(380,80),Size=new Size(100,34)};dlg.Controls.Add(edit);dlg.Controls.Add(ok);dlg.AcceptButton=ok;return dlg.ShowDialog(this)==DialogResult.OK?edit.Text:null;
    }
    async Task AddService(){using var d=new OpenFileDialog{Title="选择反代的启动程序",Filter="启动程序|*.exe"};if(d.ShowDialog(this)!=DialogResult.OK)return;var name=Prompt("服务名称",Path.GetFileNameWithoutExtension(d.FileName));if(name==null)return;var port=Prompt("服务端口","8000");if(port==null)return;var argText=Prompt("参数（JSON 数组，路径中的反斜杠写成两个）","[]");if(argText==null)return;await Program.Api("service-add",new{name,port=int.Parse(port),executable=d.FileName,args=JsonSerializer.Deserialize<string[]>(argText)??Array.Empty<string>()});await Refresh();}
    new async Task Refresh()
    {
        if(refreshing)return;refreshing=true;
        try
        {
            var s=await Program.Api("status");var today=s["now"]!["date"]!.ToString();var accounts=s["accounts"]!.AsArray().ToList();
            Populate(cn,accounts.Where(a=>a!["site"]!.ToString()=="cn"),a=>AccountFields(a,today));Populate(ai,accounts.Where(a=>a!["site"]!.ToString()=="ai"),a=>AccountFields(a,today));
            Populate(services,s["services"]!.AsArray(),j=>new[]{j["name"]!.ToString(),j["status"]!.ToString(),j["url"]!.ToString(),j["autoStart"]?.GetValue<bool>()==true?"随后台启动":"手动启动",j["description"]?.ToString()??""});
            var enabled=accounts.Where(a=>a!["enabled"]!.GetValue<bool>()).ToList();values[0].Text=enabled.Count(a=>a!["site"]!.ToString()=="cn").ToString();values[1].Text=enabled.Count(a=>a!["site"]!.ToString()=="ai").ToString();values[2].Text=enabled.Count(a=>a!["site"]!.ToString()=="cn"&&a["checkin"]?["date"]?.ToString()==today&&a["checkin"]?["success"]?.GetValue<bool>()==true)+" / "+values[0].Text;values[3].Text=s["services"]!.AsArray().Count(j=>j!["running"]!.GetValue<bool>()).ToString();
            var cfg=s["settings"]!;overview.Text=$"每日签到   {(cfg["checkinEnabled"]!.GetValue<bool>()?"开机 / 跨日自动补签 · 全自动":"已关闭")}       自启动检测   {(cfg["startupGuard"]!.GetValue<bool>()?"保护中":"已关闭")}\r\n\r\n调用地址   http://127.0.0.1:{cfg["port"]}/v1\r\n\r\n中国版      模型名-cn       国际版      模型名-ai\r\n\r\n两个号池独立轮询；未带后缀时使用{(cfg["defaultSite"]!.ToString()=="cn"?"中国版":"国际版")}。";
            var memory=s["runtime"]?["workingBytes"]?.GetValue<long>()??0;runtimeBadge.Text=memory>0?$"● 后台 {memory/1048576d:0.0} MB":"● 后台运行中";
            if(!dirty){var was=ready;ready=false;autoSign.Checked=cfg["checkinEnabled"]!.GetValue<bool>();startupGuard.Checked=cfg["startupGuard"]!.GetValue<bool>();signTime.Text=cfg["checkinTime"]!.ToString();defaultPool.SelectedIndex=cfg["defaultSite"]!.ToString()=="ai"?1:0;ready=was;}
            var l=await Program.Api("logs");var text=l["text"]!.ToString().Replace("\n",Environment.NewLine);if(logs.Text!=text){logs.Text=text;logs.SelectionStart=logs.TextLength;logs.ScrollToCaret();}if(!busy)footer.Text=s["checkinRunning"]?.GetValue<bool>()==true?"正在后台自动签到，完成后会更新账号记录":"已更新 · "+DateTime.Now.ToString("HH:mm:ss")+" · 后台服务运行中";
            if(pageTitle.Text=="公网共享"&&!busy){try{await RefreshSharing();}catch(Exception e){footer.Text="公网共享："+e.Message;}}
        }catch(Exception e){footer.Text="连接异常："+e.Message;runtimeBadge.Text="● 后台未连接";Program.Diagnostic("刷新失败 "+e.GetType().Name+" "+e.Message);}finally{refreshing=false;}
    }
    static string[] AccountFields(JsonNode a,string today){var c=a["checkin"];var isToday=c?["date"]?.ToString()==today;return new[]{a["label"]!.ToString(),string.IsNullOrEmpty(a["phoneSuffix"]?.ToString())?"—":"****"+a["phoneSuffix"],a["enabled"]!.GetValue<bool>()?a["authState"]!.ToString():"已停用",a["balance"]?.ToString()??"未查询",a["site"]!.ToString()=="ai"?"不参与":isToday?c?["status"]?.ToString()??"待签到":"待签到",isToday?c?["credit"]?.ToString()??"—":"—",a["requests"]?.ToString()??"0",a["successes"]?.ToString()??"0",a["failures"]?.ToString()??"0"};}
    static void Populate(ListView list,IEnumerable<JsonNode?> data,Func<JsonNode,string[]> fields){var selected=list.SelectedItems.Count>0?((JsonNode)list.SelectedItems[0].Tag!)["id"]?.ToString():null;list.BeginUpdate();list.Items.Clear();foreach(var j in data){if(j==null)continue;var row=new ListViewItem(fields(j)){Tag=j};list.Items.Add(row);if(j["id"]?.ToString()==selected)row.Selected=true;}list.EndUpdate();}
}

public sealed class Surface:Panel
{
    public Surface(){DoubleBuffered=true;}
    protected override void OnPaint(PaintEventArgs e){base.OnPaint(e);using var pen=new Pen(Color.FromArgb(227,232,239));e.Graphics.DrawRectangle(pen,0,0,Width-1,Height-1);}
}
